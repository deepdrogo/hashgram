package types

import (
	sdk "github.com/cosmos/cosmos-sdk/types"

	hgparams "github.com/hashgram/hashgram/app/params"
)

const (
	// DefaultRegistrationFeeHash is the fee to claim a name, in whole HASH.
	//
	// A fee is necessary rather than merely nice: a free namespace is
	// registered in its entirety by whoever scripts it first. It is small
	// enough that a real user does not notice and large enough that
	// registering a million names costs a million HASH.
	DefaultRegistrationFeeHash int64 = 1

	// DefaultRenewalFeeHash is the fee to renew, in whole HASH.
	DefaultRenewalFeeHash int64 = 1

	// DefaultRegistrationPeriodBlocks is about one year at 4 second blocks.
	//
	// Expiry exists so abandoned names return to circulation. A permanent
	// free claim rewards whoever registered fastest, forever.
	DefaultRegistrationPeriodBlocks int64 = 7_884_000

	// DefaultGracePeriodBlocks is about 30 days: long enough that a user who
	// missed a renewal is not sniped by a bot the same hour.
	DefaultGracePeriodBlocks int64 = 648_000

	// DefaultMinLength and DefaultMaxLength bound a name in code points.
	//
	// The three-character floor keeps very short names, which are the most
	// valuable and most impersonation-prone, out of circulation at launch.
	// Governance can lower it to the protocol floor of 2 later.
	DefaultMinLength uint32 = 3
	DefaultMaxLength uint32 = 32
)

// DefaultParams returns the launch configuration.
func DefaultParams() Params {
	return Params{
		RegistrationFee: sdk.NewCoins(sdk.NewCoin(
			hgparams.BaseCoinDenom, hgparams.HashToBase(DefaultRegistrationFeeHash))),
		RenewalFee: sdk.NewCoins(sdk.NewCoin(
			hgparams.BaseCoinDenom, hgparams.HashToBase(DefaultRenewalFeeHash))),
		RegistrationPeriodBlocks: DefaultRegistrationPeriodBlocks,
		GracePeriodBlocks:        DefaultGracePeriodBlocks,
		MinLength:                DefaultMinLength,
		MaxLength:                DefaultMaxLength,
		ReservedNames:            DefaultReservedNames(),

		// ASCII only at launch. An ASCII namespace has no homograph attacks
		// at all, and widening it later is a smaller decision than narrowing
		// it after names have been registered.
		AllowNonAscii: false,
	}
}

// Validate checks the parameter set.
func (p Params) Validate() error {
	if !p.RegistrationFee.IsValid() || p.RegistrationFee.IsAnyNegative() {
		return ErrInvalidParams.Wrapf("registration_fee %q is invalid", p.RegistrationFee)
	}
	if !p.RenewalFee.IsValid() || p.RenewalFee.IsAnyNegative() {
		return ErrInvalidParams.Wrapf("renewal_fee %q is invalid", p.RenewalFee)
	}
	if p.RegistrationFee.IsZero() {
		return ErrInvalidParams.Wrap(
			"registration_fee must be positive; a free namespace is registered in " +
				"its entirety by whoever scripts it first")
	}
	if p.RegistrationPeriodBlocks < 0 {
		return ErrInvalidParams.Wrap("registration_period_blocks must not be negative")
	}
	if p.GracePeriodBlocks < 0 {
		return ErrInvalidParams.Wrap("grace_period_blocks must not be negative")
	}

	if p.MinLength < MinNameLength {
		return ErrInvalidParams.Wrapf(
			"min_length %d is below the protocol floor of %d", p.MinLength, MinNameLength)
	}
	if p.MaxLength > MaxNameLength {
		return ErrInvalidParams.Wrapf(
			"max_length %d is above the protocol ceiling of %d", p.MaxLength, MaxNameLength)
	}
	if p.MinLength > p.MaxLength {
		return ErrInvalidParams.Wrapf(
			"min_length %d exceeds max_length %d", p.MinLength, p.MaxLength)
	}

	seen := make(map[string]bool, len(p.ReservedNames))
	for _, n := range p.ReservedNames {
		normalized := Normalize(n)
		if normalized == "" {
			return ErrInvalidParams.Wrapf("reserved name %q normalises to empty", n)
		}
		if seen[normalized] {
			return ErrInvalidParams.Wrapf("reserved name %q appears twice", normalized)
		}
		seen[normalized] = true
	}

	return nil
}

// IsReserved reports whether a normalised name is reserved.
//
// The check is on the skeleton as well as the name, so "adm1n" cannot be used
// to sidestep the reservation of "admin".
func (p Params) IsReserved(normalized string) bool {
	skeleton := Skeleton(normalized)
	for _, n := range p.ReservedNames {
		rn := Normalize(n)
		if rn == normalized || Skeleton(rn) == skeleton {
			return true
		}
	}
	return false
}

// DefaultGenesis returns the x/username genesis state.
func DefaultGenesis() *GenesisState {
	return &GenesisState{
		Params:        DefaultParams(),
		Registrations: nil,
	}
}

// Validate checks the genesis state.
func (gs GenesisState) Validate() error {
	if err := gs.Params.Validate(); err != nil {
		return err
	}

	seenName := make(map[string]bool, len(gs.Registrations))
	seenSkeleton := make(map[string]string, len(gs.Registrations))

	for i, r := range gs.Registrations {
		if Normalize(r.Name) != r.Name {
			return ErrInvalidName.Wrapf(
				"registrations[%d] name %q is not stored in normalised form (%q)",
				i, r.Name, Normalize(r.Name))
		}
		if err := Validate(r.Name, gs.Params.MinLength, gs.Params.MaxLength, gs.Params.AllowNonAscii); err != nil {
			return ErrInvalidName.Wrapf("registrations[%d] %q: %v", i, r.Name, err)
		}
		if _, err := sdk.AccAddressFromBech32(r.Owner); err != nil {
			return ErrInvalidName.Wrapf("registrations[%d].owner %q: %v", i, r.Owner, err)
		}
		if want := Skeleton(r.Name); r.Skeleton != want {
			return ErrInvalidName.Wrapf(
				"registrations[%d] %q records skeleton %q but folds to %q",
				i, r.Name, r.Skeleton, want)
		}
		if seenName[r.Name] {
			return ErrNameTaken.Wrapf("registrations contains %q twice", r.Name)
		}
		seenName[r.Name] = true

		// Two confusable names must not both exist, or a restored chain would
		// be more permissive than a live one.
		if other, dup := seenSkeleton[r.Skeleton]; dup {
			return ErrConfusable.Wrapf(
				"registrations contains both %q and %q, which are visually confusable",
				other, r.Name)
		}
		seenSkeleton[r.Skeleton] = r.Name

		if gs.Params.IsReserved(r.Name) {
			return ErrNameReserved.Wrapf("registrations[%d] %q is a reserved name", i, r.Name)
		}
	}

	return nil
}
