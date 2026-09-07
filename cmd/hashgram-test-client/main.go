// Command hashgram-test-client is a developer client for Hashgram.
//
// Its purpose is to demonstrate, from outside the node, that the protocol
// works: that a wallet can be created and used, that staking and rewards are
// queryable, and that the signed off-chain object formats can be produced
// correctly by a third party rather than only by the code that verifies them.
//
// That last point is why it exists rather than being folded into hashgramd.
// A signing format that only the verifying implementation can produce is a
// format nobody else can implement. Every canonical encoding here goes
// through the same exported functions the state machine uses, so if a client
// SDK reproduces what this tool does, it will interoperate.
//
// It is a developer tool. It is not the Hashgram wallet, it is not hardened
// for end users, and it must not be used to hold the Founder key.
package main

import (
	"fmt"
	"os"

	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/cmd/hashgram-test-client/cmd"
)

func main() {
	hgparams.SetSDKConfig()

	if err := cmd.NewRootCmd().Execute(); err != nil {
		fmt.Fprintln(os.Stderr, "error:", err)
		os.Exit(1)
	}
}
