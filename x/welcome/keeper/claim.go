package keeper

import (
	"context"
	"fmt"

	sdk "github.com/cosmos/cosmos-sdk/types"

	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/x/welcome/types"
)

// Claim pays a welcome reward to the subject of a valid attestation.
//
// The order of the checks is deliberate. Everything that can be decided
// without writing state is decided first, so that a rejected claim leaves no
// trace and costs the network as little as possible. State is touched only
// once the claim is certain to succeed:
//
//  1. programme enabled
//  2. attestation is well-formed
//  3. attestor is registered and enabled
//  4. attestation method matches the registered method
//  5. confidence meets the minimum
//  6. attestation has not expired, and its expiry is not absurdly far out
//  7. signature verifies against the network-domain-separated digest
//  8. the (attestor, nonce) pair has not been used
//  9. the subject has not already claimed
//  10. the attestor is under its per-epoch cap
//  11. the schedule is not exhausted
//  12. the pool can cover the reward
//     -- from here on, state is written --
//
// Note what is NOT a check: whether the subject holds a key, has ever sent a
// transaction, or exists as an account. Creating a key is free and gives you
// nothing. Eligibility comes from an attestor, and the attestor set is empty
// until governance registers one.
func (k Keeper) Claim(ctx context.Context, att types.EligibilityAttestation) (types.ClaimRecord, error) {
	sdkCtx := sdk.UnwrapSDKContext(ctx)
	height := sdkCtx.BlockHeight()

	params, err := k.GetParams(ctx)
	if err != nil {
		return types.ClaimRecord{}, err
	}
	if !params.Enabled {
		return types.ClaimRecord{}, types.ErrDisabled
	}

	if err := att.ValidateBasic(); err != nil {
		return types.ClaimRecord{}, err
	}

	attestor, found := params.Attestor(att.Attestor)
	if !found {
		return types.ClaimRecord{}, types.ErrUnknownAttestor.Wrapf(
			"%s is not a registered attestor", att.Attestor)
	}
	if !attestor.Enabled {
		return types.ClaimRecord{}, types.ErrAttestorDisabled.Wrapf("%s", att.Attestor)
	}
	if att.Method != attestor.Method {
		return types.ClaimRecord{}, types.ErrMethodMismatch.Wrapf(
			"attestation claims method %q but attestor %s is registered for %q",
			att.Method, attestor.Address, attestor.Method)
	}
	if att.Confidence < params.MinConfidence {
		return types.ClaimRecord{}, types.ErrConfidenceTooLow.Wrapf(
			"confidence %d is below the minimum of %d basis points",
			att.Confidence, params.MinConfidence)
	}

	if att.ExpiryHeight < height {
		return types.ClaimRecord{}, types.ErrAttestationExpired.Wrapf(
			"expired at height %d, current height is %d", att.ExpiryHeight, height)
	}
	if att.ExpiryHeight > height+params.MaxAttestationAgeBlocks {
		return types.ClaimRecord{}, types.ErrAttestationTooLong.Wrapf(
			"expiry height %d is %d blocks away, maximum is %d",
			att.ExpiryHeight, att.ExpiryHeight-height, params.MaxAttestationAgeBlocks)
	}

	if err := k.verifyAttestationSignature(ctx, attestor, att); err != nil {
		return types.ClaimRecord{}, err
	}

	used, err := k.IsNonceUsed(ctx, att.Attestor, att.Nonce)
	if err != nil {
		return types.ClaimRecord{}, err
	}
	if used {
		return types.ClaimRecord{}, types.ErrNonceReused.Wrapf(
			"attestor %s nonce %d", att.Attestor, att.Nonce)
	}

	claimed, err := k.HasClaimed(ctx, att.Subject)
	if err != nil {
		return types.ClaimRecord{}, err
	}
	if claimed {
		return types.ClaimRecord{}, types.ErrAlreadyClaimed.Wrapf("%s", att.Subject)
	}

	epoch := k.Epoch(height, params.EpochBlocks)
	count, err := k.AttestorEpochCount(ctx, att.Attestor, epoch)
	if err != nil {
		return types.ClaimRecord{}, err
	}
	if count >= attestor.MaxClaimsPerEpoch {
		return types.ClaimRecord{}, types.ErrAttestorCapReached.Wrapf(
			"attestor %s has authorised %d claims in epoch %d, cap is %d",
			att.Attestor, count, epoch, attestor.MaxClaimsPerEpoch)
	}

	sequence, err := k.NextSequence(ctx)
	if err != nil {
		return types.ClaimRecord{}, err
	}
	if types.IsExhausted(sequence) {
		// The programme finished. The sequence counter is deliberately not
		// advanced, so state stops growing once there is nothing left to pay.
		return types.ClaimRecord{}, types.ErrExhausted.Wrapf(
			"sequence %d is past the last funded position (%d)",
			sequence, hgparams.WelcomeTier3Limit)
	}

	amount := types.TierAmount(sequence)
	if amount.IsZero() {
		return types.ClaimRecord{}, types.ErrExhausted.Wrapf("sequence %d pays nothing", sequence)
	}

	pool := k.PoolRemaining(ctx)
	if !pool.IsAllGTE(amount) {
		// Should be unreachable: the pool is funded with the schedule's
		// worst-case total. Checked anyway, because paying out of an
		// underfunded pool would fail inside x/bank with a much less
		// informative error.
		return types.ClaimRecord{}, types.ErrPoolInsufficient.Wrapf(
			"pool holds %s, reward for sequence %d is %s", pool, sequence, amount)
	}

	subject, err := sdk.AccAddressFromBech32(att.Subject)
	if err != nil {
		return types.ClaimRecord{}, types.ErrInvalidAttestation.Wrapf("subject %q: %v", att.Subject, err)
	}
	if k.bankKeeper.BlockedAddr(subject) {
		return types.ClaimRecord{}, types.ErrInvalidAttestation.Wrapf(
			"subject %s is a blocked address and cannot receive funds", att.Subject)
	}

	// --- state writes begin here ---

	if _, err := k.advanceSequence(ctx); err != nil {
		return types.ClaimRecord{}, err
	}
	if err := k.MarkNonceUsed(ctx, att.Attestor, att.Nonce); err != nil {
		return types.ClaimRecord{}, err
	}
	if err := k.incrementAttestorEpochCount(ctx, att.Attestor, epoch); err != nil {
		return types.ClaimRecord{}, err
	}

	if err := k.bankKeeper.SendCoinsFromModuleToAccount(ctx, types.ModuleName, subject, amount); err != nil {
		return types.ClaimRecord{}, fmt.Errorf("x/welcome: paying %s to %s: %w", amount, subject, err)
	}

	record := types.ClaimRecord{
		Subject:  att.Subject,
		Sequence: sequence,
		Amount:   amount,
		Height:   height,
		Attestor: att.Attestor,
		Method:   att.Method,
	}
	if err := k.SetClaim(ctx, record); err != nil {
		return types.ClaimRecord{}, err
	}
	if err := k.addTotalPaid(ctx, amount); err != nil {
		return types.ClaimRecord{}, err
	}
	if _, err := k.claimsPaid.Next(ctx); err != nil {
		return types.ClaimRecord{}, err
	}

	sdkCtx.EventManager().EmitEvent(sdk.NewEvent(
		types.EventTypeWelcomeClaimed,
		sdk.NewAttribute(types.AttributeKeySubject, att.Subject),
		sdk.NewAttribute(types.AttributeKeySequence, fmt.Sprintf("%d", sequence)),
		sdk.NewAttribute(types.AttributeKeyAmount, amount.String()),
		sdk.NewAttribute(types.AttributeKeyAttestor, att.Attestor),
		sdk.NewAttribute(types.AttributeKeyMethod, att.Method),
		sdk.NewAttribute(types.AttributeKeyConfidence, fmt.Sprintf("%d", att.Confidence)),
	))

	return record, nil
}

// verifyAttestationSignature checks the attestation signature against the
// network-domain-separated digest.
//
// The digest comes from x/network, which mixes in the network magic, the
// protocol major version, the network id and the "eligibility-attestation"
// purpose. Consequences: an attestation minted on a devnet or on a fork does
// not verify here, and these bytes cannot be reinterpreted as a service
// receipt or a device certificate.
func (k Keeper) verifyAttestationSignature(
	ctx context.Context,
	attestor types.Attestor,
	att types.EligibilityAttestation,
) error {
	payload := types.CanonicalAttestationBytes(att)

	digest, err := k.networkKeeper.SigningDigest(ctx, hgparams.PurposeEligibility, payload)
	if err != nil {
		return err
	}
	return attestor.VerifySignature(digest[:], att.Signature)
}

// AttestationDigest exposes the digest an attestor must sign.
//
// Used by the developer test client and by client SDKs so that no attestor
// implementation has to reproduce the canonical encoding by hand and get it
// subtly wrong.
func (k Keeper) AttestationDigest(ctx context.Context, att types.EligibilityAttestation) ([32]byte, error) {
	return k.networkKeeper.SigningDigest(ctx, hgparams.PurposeEligibility,
		types.CanonicalAttestationBytes(att))
}
