package cmd

import (
	"fmt"
	"os"
	"os/user"
	"path/filepath"
	"strconv"
	"syscall"

	"github.com/hashgram/hashgram/cmd/hashgramctl/internal/hgconfig"
)

// chainServiceUser is the account hashgramd.service runs as. It is created
// by scripts/install/bootstrap-ubuntu.sh and named in deploy/systemd.
const chainServiceUser = "hashgram-chain"

// productionNodeHome is where hashgramd.service keeps its state. When this
// directory exists, the machine was bootstrapped for production and every
// hashgramctl command should operate on it rather than on the invoking
// user's dotfile directory: an operator who initialises /root/.hashgram and
// then starts a service that reads /var/lib/hashgram/chain has two nodes'
// worth of configuration and none of it where the service looks.
func productionNodeHome(dataDir string) (string, bool) {
	home := filepath.Join(dataDir, "chain")
	if st, err := os.Stat(home); err == nil && st.IsDir() {
		return home, true
	}
	return "", false
}

// adoptServiceOwnership hands the node home to the chain service account
// after a command run as root has written into it.
//
// hashgramd.service runs as hashgram-chain with a 0750 home. Files root
// writes there — genesis.json, the node key, app.toml, the validator key on
// first init — are root-owned, and the service cannot read or rotate them.
// Rather than telling the operator to remember a chown at the most
// consequential step of the launch, the commands that write do it.
//
// A no-op unless running as root, the service account exists, and the node
// home is the production one; a developer's ~/.hashgram is never touched.
func adoptServiceOwnership(p hgconfig.Paths) error {
	if os.Geteuid() != 0 {
		return nil
	}
	if prod, ok := productionNodeHome(p.DataDir); !ok || prod != p.NodeHome {
		return nil
	}
	u, err := user.Lookup(chainServiceUser)
	if err != nil {
		return nil // not a bootstrapped host
	}
	uid, err := strconv.Atoi(u.Uid)
	if err != nil {
		return err
	}
	gid, err := strconv.Atoi(u.Gid)
	if err != nil {
		return err
	}
	return filepath.WalkDir(p.NodeHome, func(path string, _ os.DirEntry, walkErr error) error {
		if walkErr != nil {
			return walkErr
		}
		st, err := os.Lstat(path)
		if err != nil {
			return err
		}
		if sys, ok := st.Sys().(*syscall.Stat_t); ok && int(sys.Uid) == uid && int(sys.Gid) == gid {
			return nil
		}
		if err := os.Lchown(path, uid, gid); err != nil {
			return fmt.Errorf("handing %s to %s: %w", path, chainServiceUser, err)
		}
		return nil
	})
}
