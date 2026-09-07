package model

import (
	"cosmossdk.io/math"

	hgparams "github.com/hashgram/hashgram/app/params"
	welcometypes "github.com/hashgram/hashgram/x/welcome/types"
)

// welcomeSegment is a run of consecutive sequence numbers that all receive
// the same reward.
type welcomeSegment struct {
	// Count is how many sequence numbers are in the segment.
	Count int64
	// AmountPerClaim is the reward for each of them, in uhash.
	AmountPerClaim math.Int
}

// welcomeSegmentsFor splits the sequence range (from, from+count] into runs
// of equal reward.
//
// Iterating once per joining user would be the obvious implementation and is
// unusable: a scenario with a billion users would need a billion iterations
// per epoch. Because the tier schedule is piecewise constant with three
// pieces, the same result is obtained in at most four steps.
//
// The reward for each segment still comes from the chain's TierAmountHash, and
// the segment boundaries come from the chain's Tiers(), so this remains a
// batching of the real schedule rather than a second copy of it.
func welcomeSegmentsFor(from uint64, count int64) []welcomeSegment {
	if count <= 0 {
		return nil
	}

	var (
		out       []welcomeSegment
		seq       = from + 1
		remaining = count
	)

	for remaining > 0 {
		amountHash := welcometypes.TierAmountHash(seq)
		if amountHash <= 0 {
			// Past the last funded tier. Every later sequence is zero too,
			// because the schedule is monotonically non-increasing, so there
			// is nothing further to account for.
			break
		}

		end := tierEndFor(seq)
		// #nosec G115 -- end is a tier boundary at or above seq, and the tier
		// boundaries are compile-time constants in the low millions.
		span := int64(end - seq + 1)
		if span <= 0 || span > remaining {
			span = remaining
		}

		out = append(out, welcomeSegment{
			Count:          span,
			AmountPerClaim: hgparams.HashToBase(amountHash),
		})

		seq += uint64(span)
		remaining -= span
	}

	return out
}

// tierEndFor returns the last sequence number in the tier containing seq.
//
// Derived from the chain's Tiers() rather than from restated constants, so
// that a future change to the schedule shape is picked up here automatically.
func tierEndFor(seq uint64) uint64 {
	for _, t := range welcometypes.Tiers() {
		if seq <= t.MaxSequence {
			return t.MaxSequence
		}
	}
	// Past every tier. The caller has already checked TierAmountHash and will
	// not reach this, but returning seq keeps the span at one rather than
	// looping forever if that ever changes.
	return seq
}

// payWelcomeBatch pays the welcome rewards for count users starting after
// sequence number from, and reports the total paid and how many users were
// actually funded.
//
// The Welcome pool cap is applied by Ledger.PayWelcome, so a batch that spans
// the point of exhaustion is settled partially and the funded count reflects
// what was really paid rather than what was requested.
func payWelcomeBatch(l *Ledger, from uint64, count int64) (math.Int, int64, error) {
	total := math.ZeroInt()
	funded := int64(0)

	for _, seg := range welcomeSegmentsFor(from, count) {
		want := seg.AmountPerClaim.MulRaw(seg.Count)
		paid, err := l.PayWelcome(want)
		if err != nil {
			return math.ZeroInt(), 0, err
		}
		if !paid.IsPositive() {
			// The pool is exhausted; later segments pay less, never more.
			break
		}

		total = total.Add(paid)
		funded += paid.Quo(seg.AmountPerClaim).Int64()

		if paid.LT(want) {
			// Partial settlement means the pool ran out inside this segment.
			break
		}
	}

	return total, funded, nil
}
