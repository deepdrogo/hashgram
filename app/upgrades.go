package app

import (
	"context"

	upgradetypes "cosmossdk.io/x/upgrade/types"

	"github.com/cosmos/cosmos-sdk/types/module"

	"github.com/hashgram/hashgram/app/upgrades"
)

// appUpgrade pairs a declarative upgrade record with an optional imperative
// step that needs access to the live keepers.
type appUpgrade struct {
	upgrades.Upgrade

	// PreRun runs before module migrations. Use it only for state fixes that
	// cannot be expressed as a module consensus-version migration.
	PreRun func(ctx context.Context, app *HashgramApp) error
}

// registeredUpgrades returns the upgrades this binary can perform.
//
// The declarative half comes from app/upgrades.Registry so that tooling can
// list upgrades without importing the whole application.
func registeredUpgrades() []appUpgrade {
	declared := upgrades.Registry()
	out := make([]appUpgrade, 0, len(declared))
	for _, u := range declared {
		out = append(out, appUpgrade{Upgrade: u})
	}
	return out
}

// RegisterUpgradeHandlers registers every on-chain upgrade this binary knows
// how to perform.
//
// Upgrades are the only mechanism by which the Hashgram state machine changes
// after launch. Each one requires a governance proposal that the validator set
// votes on, plus operators actually running the new binary. There is no path
// by which a single party performs an upgrade, and no "founder override"
// exists here or anywhere else.
//
// At genesis there are no upgrades to register.
func (app *HashgramApp) RegisterUpgradeHandlers() {
	all := registeredUpgrades()

	for _, u := range all {
		u := u
		app.UpgradeKeeper.SetUpgradeHandler(
			u.Name,
			func(ctx context.Context, _ upgradetypes.Plan, fromVM module.VersionMap) (module.VersionMap, error) {
				if u.PreRun != nil {
					if err := u.PreRun(ctx, app); err != nil {
						return nil, err
					}
				}
				return app.ModuleManager.RunMigrations(ctx, app.Configurator(), fromVM)
			},
		)
	}

	if len(all) == 0 {
		return
	}

	upgradeInfo, err := app.UpgradeKeeper.ReadUpgradeInfoFromDisk()
	if err != nil {
		panic(err)
	}
	if upgradeInfo.Name == "" || app.UpgradeKeeper.IsSkipHeight(upgradeInfo.Height) {
		return
	}

	for _, u := range all {
		if upgradeInfo.Name != u.Name {
			continue
		}
		su := u.StoreUpgrades
		if len(su.Added) == 0 && len(su.Renamed) == 0 && len(su.Deleted) == 0 {
			continue
		}
		// Applies the store migration exactly at the upgrade height.
		app.SetStoreLoader(upgradetypes.UpgradeStoreLoader(upgradeInfo.Height, &su))
	}
}
