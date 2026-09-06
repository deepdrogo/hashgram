// Package upgrades declares the registry of named on-chain upgrades.
//
// It lives in its own package so that a release can add an upgrade without
// touching app wiring, and so that the registry can be inspected by tooling
// (`hashgramctl update` prints the upgrades a binary knows about).
package upgrades

import (
	storetypes "cosmossdk.io/store/types"
)

// Upgrade describes one named on-chain upgrade.
type Upgrade struct {
	// Name must match the MsgSoftwareUpgrade plan name exactly.
	Name string

	// StoreUpgrades lists store keys added, renamed or deleted by this
	// upgrade. Getting this wrong is a consensus failure at the upgrade
	// height, so it is stated explicitly rather than inferred.
	StoreUpgrades storetypes.StoreUpgrades

	// Notes is human-readable release documentation, surfaced by
	// `hashgramctl update`.
	Notes string
}

// Registry is the ordered list of upgrades this binary can perform.
//
// It is empty at genesis. Appending an entry here is a state-machine breaking
// change and must ship with a version bump and a governance proposal.
func Registry() []Upgrade {
	return nil
}
