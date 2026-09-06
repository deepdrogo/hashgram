// Command hashgramd is the Hashgram blockchain node.
//
// It is the consensus-critical binary: it validates blocks, serves RPC, and
// (when configured with a validator key) signs. Operators normally drive it
// through hashgramctl rather than directly.
package main

import (
	"fmt"
	"os"

	svrcmd "github.com/cosmos/cosmos-sdk/server/cmd"

	"github.com/hashgram/hashgram/app"
	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/cmd/hashgramd/cmd"
)

func main() {
	// Address prefixes and the HD coin type must be installed before any
	// bech32 string is parsed, which means before cobra builds its commands.
	hgparams.SetSDKConfig()

	rootCmd := cmd.NewRootCmd()
	if err := svrcmd.Execute(rootCmd, "HASHGRAM", app.DefaultNodeHome); err != nil {
		fmt.Fprintln(rootCmd.OutOrStderr(), err)
		os.Exit(1)
	}
}
