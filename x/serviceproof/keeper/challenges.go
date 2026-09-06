package keeper

import (
	"context"
	"fmt"

	"cosmossdk.io/collections"

	sdk "github.com/cosmos/cosmos-sdk/types"

	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/x/serviceproof/types"
)

// IssueChallenges issues storage challenges to providers with active
// assignments.
//
// Challenges are issued by the chain, never requested by the provider, and
// the chunk index is derived from the block's app hash. A provider deciding
// whether to actually keep the data cannot predict what will be asked, so
// keeping only part of it reduces expected reward in proportion while doing
// nothing to reduce the risk of a fraud score.
//
// Called at the start of each epoch. Providers with no assignments receive no
// challenges, which is also why declared-but-unassigned disk earns nothing:
// there is no evidence to produce.
func (k Keeper) IssueChallenges(ctx sdk.Context, epoch uint64, params types.Params) error {
	appHash := ctx.BlockHeader().AppHash
	height := ctx.BlockHeight()
	deadline := height + params.ChallengeResponseBlocks

	var issueErr error
	err := k.IterateProviders(ctx, func(p types.Provider) (bool, error) {
		if !p.IsActive() || !p.HasRole(types.SERVICE_ROLE_STORAGE) {
			return false, nil
		}

		active, err := k.activeAssignments(ctx, p.Operator)
		if err != nil {
			return true, err
		}
		if len(active) == 0 {
			return false, nil
		}

		// Challenge up to challenges_per_epoch assignments, chosen by the
		// same unpredictable entropy as the chunk index.
		n := int(params.ChallengesPerEpoch)
		if n > len(active) {
			n = len(active)
		}

		for i := 0; i < n; i++ {
			id, err := k.nextChallengeID.Next(ctx)
			if err != nil {
				return true, err
			}
			// nextChallengeID starts at 0; challenge ids start at 1 so that
			// zero remains an unambiguous "no challenge".
			id++

			pick := types.ChallengeChunkIndex(appHash, id, []byte(p.Operator), uint32(i), uint32(len(active)))
			a := active[pick]

			chunkIndex := types.ChallengeChunkIndex(appHash, id, a.BlobId, a.ReplicaIndex, a.ChunkCount)

			ch := types.StorageChallenge{
				Id:             id,
				Provider:       p.Operator,
				BlobId:         a.BlobId,
				ReplicaIndex:   a.ReplicaIndex,
				ChunkIndex:     chunkIndex,
				IssuedHeight:   height,
				IssuedEpoch:    epoch,
				DeadlineHeight: deadline,
			}
			if err := k.challenges.Set(ctx, id, ch); err != nil {
				return true, err
			}

			credit, err := k.GetCredit(ctx, epoch, p.Operator)
			if err != nil {
				return true, err
			}
			credit.ChallengesIssued++
			if err := k.SetCredit(ctx, credit); err != nil {
				return true, err
			}

			ctx.EventManager().EmitEvent(sdk.NewEvent(
				types.EventTypeChallengeIssued,
				sdk.NewAttribute(types.AttributeKeyChallengeID, fmt.Sprintf("%d", id)),
				sdk.NewAttribute(types.AttributeKeyProvider, p.Operator),
				sdk.NewAttribute(types.AttributeKeyBlobID, fmt.Sprintf("%x", a.BlobId)),
			))
		}
		return false, nil
	})
	if err != nil {
		return err
	}
	return issueErr
}

// AnswerChallenge verifies a provider's Merkle proof.
//
// Two things are checked, and both are necessary:
//
//   - The Merkle path must verify against the root recorded in the
//     assignment. This is what makes the proof mean "I hold this chunk"
//     rather than "I can produce 32 bytes".
//   - The response must be signed by the provider's node key. Without the
//     signature, a proof observed on chain could be replayed by anyone; the
//     signature ties the answer to the node that was actually challenged.
//
// The chain never sees chunk contents. A provider proving it holds a 4 MiB
// chunk submits a 32-byte hash and a path of about twenty hashes, not the
// chunk.
func (k Keeper) AnswerChallenge(ctx context.Context, operator string, resp types.ChallengeResponse) (bool, error) {
	params, err := k.GetParams(ctx)
	if err != nil {
		return false, err
	}

	sdkCtx := sdk.UnwrapSDKContext(ctx)
	height := sdkCtx.BlockHeight()

	ch, err := k.challenges.Get(ctx, resp.ChallengeId)
	if err != nil {
		return false, types.ErrChallengeNotFound.Wrapf("challenge %d", resp.ChallengeId)
	}
	if ch.Provider != operator {
		return false, types.ErrChallengeNotFound.Wrapf(
			"challenge %d was issued to %s, not %s", resp.ChallengeId, ch.Provider, operator)
	}
	if ch.Answered {
		return false, types.ErrChallengeAnswered.Wrapf("challenge %d", resp.ChallengeId)
	}
	if height > ch.DeadlineHeight {
		return false, types.ErrChallengeExpired.Wrapf(
			"challenge %d deadline was height %d, now %d", resp.ChallengeId, ch.DeadlineHeight, height)
	}

	provider, err := k.GetProvider(ctx, operator)
	if err != nil {
		return false, err
	}

	assignment, err := k.assignments.Get(ctx, collections.Join3(operator, ch.BlobId, ch.ReplicaIndex))
	if err != nil {
		return false, types.ErrAssignmentNotFound.Wrapf(
			"assignment for blob %x replica %d", ch.BlobId, ch.ReplicaIndex)
	}

	// Verify the node signature first. A wrong signature means we are not
	// talking to the challenged node at all, which is a different failure
	// from "the node no longer has the data".
	digest, err := k.networkKeeper.SigningDigest(ctx, hgparams.PurposeStorageChallenge,
		types.CanonicalChallengeResponseBytes(resp.ChallengeId, resp.ChunkHash, resp.MerklePath))
	if err != nil {
		return false, err
	}
	nodePub, err := types.PubKeyFromBytes(provider.NodePubkey, nodeKeyType(provider))
	if err != nil {
		return false, err
	}
	if !nodePub.VerifySignature(digest[:], resp.NodeSignature) {
		return false, types.ErrInvalidSignature.Wrap(
			"challenge response is not signed by the provider's registered node key")
	}

	proofErr := types.VerifyMerkleProof(
		assignment.MerkleRoot, resp.ChunkHash, resp.MerklePath,
		ch.ChunkIndex, assignment.ChunkCount,
	)

	ch.Answered = true
	ch.Passed = proofErr == nil
	if err := k.challenges.Set(ctx, ch.Id, ch); err != nil {
		return false, err
	}

	credit, err := k.GetCredit(ctx, ch.IssuedEpoch, operator)
	if err != nil {
		return false, err
	}
	if ch.Passed {
		credit.ChallengesPassed++
	}
	if err := k.SetCredit(ctx, credit); err != nil {
		return false, err
	}

	if !ch.Passed {
		k.addFraudScore(ctx, &provider, types.FraudReasonChallengeFailed,
			fmt.Sprintf("challenge %d: %v", ch.Id, proofErr),
			types.FraudScoreForChallengeFailure, params)
		if err := k.SetProvider(ctx, provider); err != nil {
			return false, err
		}
	}

	sdkCtx.EventManager().EmitEvent(sdk.NewEvent(
		types.EventTypeChallengeAnswered,
		sdk.NewAttribute(types.AttributeKeyChallengeID, fmt.Sprintf("%d", ch.Id)),
		sdk.NewAttribute(types.AttributeKeyProvider, operator),
		sdk.NewAttribute(types.AttributeKeyPassed, fmt.Sprintf("%t", ch.Passed)),
	))

	if !ch.Passed {
		return false, types.ErrInvalidMerkleProof.Wrapf("%v", proofErr)
	}
	return true, nil
}

// ExpireChallenges marks unanswered challenges past their deadline as failed.
//
// A missed challenge counts as a failure, not as a non-event. Otherwise a
// provider could simply ignore every challenge and keep earning: silence
// would be free.
func (k Keeper) ExpireChallenges(ctx sdk.Context, params types.Params) error {
	height := ctx.BlockHeight()

	type expired struct {
		challenge types.StorageChallenge
	}
	var toExpire []expired

	if err := k.challenges.Walk(ctx, nil, func(_ uint64, ch types.StorageChallenge) (bool, error) {
		if !ch.Answered && height > ch.DeadlineHeight {
			toExpire = append(toExpire, expired{challenge: ch})
		}
		return false, nil
	}); err != nil {
		return err
	}

	for _, e := range toExpire {
		ch := e.challenge
		ch.Answered = true
		ch.Passed = false
		if err := k.challenges.Set(ctx, ch.Id, ch); err != nil {
			return err
		}

		provider, err := k.GetProvider(ctx, ch.Provider)
		if err != nil {
			// The provider was removed; nothing to score.
			continue
		}
		k.addFraudScore(ctx, &provider, types.FraudReasonChallengeMissed,
			fmt.Sprintf("challenge %d unanswered by height %d", ch.Id, ch.DeadlineHeight),
			types.FraudScoreForChallengeFailure, params)
		if err := k.SetProvider(ctx, provider); err != nil {
			return err
		}

		ctx.EventManager().EmitEvent(sdk.NewEvent(
			types.EventTypeChallengeMissed,
			sdk.NewAttribute(types.AttributeKeyChallengeID, fmt.Sprintf("%d", ch.Id)),
			sdk.NewAttribute(types.AttributeKeyProvider, ch.Provider),
		))
	}
	return nil
}

// OpenChallenges returns a provider's unanswered challenges.
func (k Keeper) OpenChallenges(ctx context.Context, operator string) ([]types.StorageChallenge, error) {
	var out []types.StorageChallenge
	err := k.challenges.Walk(ctx, nil, func(_ uint64, ch types.StorageChallenge) (bool, error) {
		if ch.Provider == operator && !ch.Answered {
			out = append(out, ch)
		}
		return false, nil
	})
	return out, err
}

// AllChallenges returns every challenge record, for genesis export.
func (k Keeper) AllChallenges(ctx context.Context) ([]types.StorageChallenge, error) {
	var out []types.StorageChallenge
	err := k.challenges.Walk(ctx, nil, func(_ uint64, ch types.StorageChallenge) (bool, error) {
		if !ch.Answered {
			out = append(out, ch)
		}
		return false, nil
	})
	return out, err
}

// SetChallenge writes a challenge record, used by genesis.
func (k Keeper) SetChallenge(ctx context.Context, ch types.StorageChallenge) error {
	return k.challenges.Set(ctx, ch.Id, ch)
}

// SetNextChallengeID sets the challenge id counter, used by genesis.
func (k Keeper) SetNextChallengeID(ctx context.Context, id uint64) error {
	if id == 0 {
		return nil
	}
	return k.nextChallengeID.Set(ctx, id-1)
}

// NextChallengeID returns the next challenge id that would be issued.
func (k Keeper) NextChallengeID(ctx context.Context) (uint64, error) {
	n, err := k.nextChallengeID.Peek(ctx)
	if err != nil {
		return 0, err
	}
	return n + 1, nil
}

// nodeKeyType infers a provider's node key scheme from the key length.
//
// The registration message carries the type explicitly and validates it, but
// the stored Provider keeps only the bytes. Length is unambiguous here
// because the two supported schemes have different sizes (33 vs 32).
func nodeKeyType(p types.Provider) types.KeyType {
	switch len(p.NodePubkey) {
	case 33:
		return types.KEY_TYPE_SECP256K1
	case 32:
		return types.KEY_TYPE_ED25519
	default:
		return types.KEY_TYPE_UNSPECIFIED
	}
}
