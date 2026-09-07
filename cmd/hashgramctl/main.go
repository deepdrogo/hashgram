// Command hashgramctl is the Hashgram node operator CLI.
//
// It is the tool an operator actually uses: it creates or joins a network,
// configures node roles, drives the systemd services, reports status, runs
// the pre-launch safety checks, and takes backups.
//
// It deliberately holds no keys and performs no signing. Key operations go
// through hashgramd's keyring or, for the Founder, through a wallet on a
// machine that is not this server.
package main

import (
	"fmt"
	"os"

	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/cmd/hashgramctl/cmd"
)

func main() {
	// Address prefixes must be installed before any bech32 string is parsed.
	hgparams.SetSDKConfig()

	if err := cmd.NewRootCmd().Execute(); err != nil {
		fmt.Fprintln(os.Stderr, "error:", err)
		os.Exit(1)
	}
}
