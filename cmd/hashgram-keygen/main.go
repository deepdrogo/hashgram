// Command hashgram-keygen creates and inspects Hashgram keys offline.
//
// This binary exists for one purpose: to let the Founder, and anyone else who
// wants a cold wallet, generate a Hashgram address on a machine that is not a
// Hashgram server. It performs no networking of any kind. There is no code
// path in it that opens a socket, resolves a hostname, or reads an
// environment variable containing a URL.
//
// It deliberately does not write a keyring. A file it wrote would be a file
// somebody could copy, and the whole point of a cold wallet is that the
// secret exists in exactly one place the operator chose. The mnemonic is
// printed once, to the terminal, and then forgotten.
//
// See docs/FOUNDER_LAUNCH_RUNBOOK.md Part A.
package main

import (
	"bufio"
	"encoding/hex"
	"fmt"
	"os"
	"strings"

	"github.com/spf13/cobra"

	"github.com/cosmos/cosmos-sdk/crypto/hd"
	"github.com/cosmos/cosmos-sdk/crypto/keys/secp256k1"
	"github.com/cosmos/go-bip39"

	sdk "github.com/cosmos/cosmos-sdk/types"

	hgparams "github.com/hashgram/hashgram/app/params"
)

func main() {
	hgparams.SetSDKConfig()

	root := &cobra.Command{
		Use:   "hashgram-keygen",
		Short: "Create and inspect Hashgram keys offline",
		Long: `Create and inspect Hashgram keys offline.

This tool performs no networking. Run it on a machine that is not a Hashgram
server, ideally one that has never been connected to a network, to create the
Founder cold wallet or any other key you want to keep off your servers.

It writes no keyring and no key file. The mnemonic is printed once and then
forgotten: a file this tool wrote would be a file somebody could copy, and the
point of a cold wallet is that the secret exists only where you put it.

  hashgram-keygen new                    create a new key
  hashgram-keygen derive                 recover the address from a mnemonic
  hashgram-keygen address <pubkey-hex>   show the address for a public key

See docs/FOUNDER_LAUNCH_RUNBOOK.md Part A for the full procedure, including
hardware wallet and multisig options, which are better than this tool for an
allocation of this size.`,
		SilenceUsage: true,
	}

	root.AddCommand(cmdNew(), cmdDerive(), cmdAddress())

	if err := root.Execute(); err != nil {
		fmt.Fprintln(os.Stderr, "error:", err)
		os.Exit(1)
	}
}

func cmdNew() *cobra.Command {
	var (
		words   int
		account uint32
		index   uint32
		quiet   bool
	)

	cmd := &cobra.Command{
		Use:   "new",
		Short: "Create a new Hashgram key and print its mnemonic once",
		Long: `Create a new Hashgram key and print its mnemonic once.

The mnemonic is the key. Anyone who reads it controls the funds, and nobody
can help you if you lose it: there is no administrator, no password reset and
no support channel that can recover a Hashgram address.

Before you run this:

  - Use a machine that is not a server and is not running anything else.
  - Make sure nothing is recording your screen or your terminal scrollback.
  - Have pen and paper ready. Do not photograph the mnemonic, do not type it
    into a password manager that syncs, and do not store it on the machine
    that generated it.

For the Founder allocation specifically, a hardware wallet or a multisig is a
better choice than a written mnemonic. See docs/FOUNDER_LAUNCH_RUNBOOK.md.`,
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			bits := 256
			if words == 12 {
				bits = 128
			} else if words != 24 {
				return fmt.Errorf("--words must be 12 or 24, got %d", words)
			}

			if !quiet {
				printWarning(cmd.OutOrStdout())
				if err := confirmTyped("I UNDERSTAND"); err != nil {
					return err
				}
			}

			entropy, err := bip39.NewEntropy(bits)
			if err != nil {
				return fmt.Errorf("generating entropy: %w", err)
			}
			mnemonic, err := bip39.NewMnemonic(entropy)
			if err != nil {
				return fmt.Errorf("generating mnemonic: %w", err)
			}

			addr, pubHex, path, err := derive(mnemonic, "", account, index)
			if err != nil {
				return err
			}

			printKey(cmd.OutOrStdout(), mnemonic, addr, pubHex, path, words)
			return nil
		},
	}

	cmd.Flags().IntVar(&words, "words", 24, "mnemonic length: 24 or 12")
	cmd.Flags().Uint32Var(&account, "account", 0, "BIP-44 account index")
	cmd.Flags().Uint32Var(&index, "index", 0, "BIP-44 address index")
	cmd.Flags().BoolVar(&quiet, "quiet", false, "skip the confirmation prompt")

	return cmd
}

func cmdDerive() *cobra.Command {
	var (
		account    uint32
		index      uint32
		passphrase string
	)

	cmd := &cobra.Command{
		Use:   "derive",
		Short: "Recover the address for a mnemonic you already have",
		Long: `Recover the address for a mnemonic you already have.

Use this to verify a backup: type in the mnemonic you wrote down and check
that it produces the address you expect. Doing that once, before you rely on
the backup, is the difference between having a backup and believing you have
one.

Comparing the address is the verification that matters. The BIP-39 checksum
catches a word that is not in the word list, and catches most single-word
substitutions, but it is only eight bits: roughly one wrong-word transcription
in 256 still produces a structurally valid mnemonic for a different key. Only
the address tells you whether you wrote down the right words.

The mnemonic is read from standard input and is never echoed.`,
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			fmt.Fprint(cmd.OutOrStdout(),
				"Enter the mnemonic (it will not be echoed to the screen), then press Enter:\n> ")

			reader := bufio.NewReader(os.Stdin)
			line, err := reader.ReadString('\n')
			if err != nil {
				return fmt.Errorf("reading the mnemonic: %w", err)
			}
			mnemonic := strings.Join(strings.Fields(line), " ")
			if mnemonic == "" {
				return fmt.Errorf("no mnemonic supplied")
			}
			if !bip39.IsMnemonicValid(mnemonic) {
				return fmt.Errorf(
					"that is not a valid BIP-39 mnemonic.\n\n" +
						"Either a word is not in the BIP-39 word list, or the checksum does not " +
						"match. Check your transcription word by word")
			}

			addr, pubHex, path, err := derive(mnemonic, passphrase, account, index)
			if err != nil {
				return err
			}

			w := cmd.OutOrStdout()
			fmt.Fprintf(w, "\n  Address        %s\n", addr)
			fmt.Fprintf(w, "  Public key     %s\n", pubHex)
			fmt.Fprintf(w, "  Derivation     %s\n\n", path)
			fmt.Fprintf(w, "  If this is not the address you expected, the mnemonic, the passphrase\n")
			fmt.Fprintf(w, "  or the account/index differs from what you used originally.\n\n")
			return nil
		},
	}

	cmd.Flags().Uint32Var(&account, "account", 0, "BIP-44 account index")
	cmd.Flags().Uint32Var(&index, "index", 0, "BIP-44 address index")
	cmd.Flags().StringVar(&passphrase, "passphrase", "",
		"BIP-39 passphrase, if one was used originally")

	return cmd
}

func cmdAddress() *cobra.Command {
	return &cobra.Command{
		Use:   "address [pubkey-hex]",
		Short: "Show the Hashgram address for a compressed secp256k1 public key",
		Long: `Show the Hashgram address for a compressed secp256k1 public key.

Useful for verifying that a public key exported from a hardware wallet
corresponds to the address you are about to put in a genesis file, without
ever touching the private key.`,
		Args: cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			raw, err := decodeHex(args[0])
			if err != nil {
				return err
			}
			if len(raw) != secp256k1.PubKeySize {
				return fmt.Errorf(
					"a compressed secp256k1 public key is %d bytes, got %d",
					secp256k1.PubKeySize, len(raw))
			}
			pub := &secp256k1.PubKey{Key: raw}
			addr := sdk.AccAddress(pub.Address())

			fmt.Fprintf(cmd.OutOrStdout(), "\n  Address     %s\n\n", addr.String())
			return nil
		},
	}
}

// derive computes the address and public key for a mnemonic.
//
// Uses the standard Cosmos BIP-44 path with coin type 118, which is what
// hardware wallets and every Cosmos-compatible wallet expect. Choosing a
// custom coin type would have meant the Founder could not use a Ledger.
func derive(mnemonic, passphrase string, account, index uint32) (address, pubkeyHex, path string, err error) {
	hdPath := hd.CreateHDPath(hgparams.BIP44CoinType, account, index)

	master, ch := hd.ComputeMastersFromSeed(bip39SeedFromMnemonic(mnemonic, passphrase))
	privBytes, err := hd.DerivePrivateKeyForPath(master, ch, hdPath.String())
	if err != nil {
		return "", "", "", fmt.Errorf("deriving key for %s: %w", hdPath.String(), err)
	}

	priv := &secp256k1.PrivKey{Key: privBytes}
	pub := priv.PubKey()

	return sdk.AccAddress(pub.Address()).String(),
		fmt.Sprintf("%x", pub.Bytes()),
		hdPath.String(),
		nil
}

func bip39SeedFromMnemonic(mnemonic, passphrase string) []byte {
	return bip39.NewSeed(mnemonic, passphrase)
}

// decodeHex parses a public key given on the command line.
//
// encoding/hex rather than a hand-rolled Sscanf loop. The loop worked, but it
// accepted whatever Sscanf's %02x accepted, which includes forms a reader
// would not predict, and it needed an int-to-byte narrowing that no reader
// can verify at a glance. The standard library validates the alphabet
// strictly and returns a typed error naming the offending byte.
func decodeHex(s string) ([]byte, error) {
	s = strings.TrimPrefix(strings.TrimSpace(s), "0x")
	if len(s)%2 != 0 {
		return nil, fmt.Errorf("hex input has an odd number of characters (%d)", len(s))
	}

	out, err := hex.DecodeString(s)
	if err != nil {
		return nil, fmt.Errorf("not valid hex: %w", err)
	}
	return out, nil
}

func printWarning(w interface{ Write([]byte) (int, error) }) {
	fmt.Fprintf(w, "\n%s\n", strings.Repeat("=", 74))
	fmt.Fprintf(w, "  CREATING A HASHGRAM KEY\n")
	fmt.Fprintf(w, "%s\n\n", strings.Repeat("=", 74))
	fmt.Fprintf(w, "  The mnemonic this prints IS the key. Anyone who reads it controls the\n")
	fmt.Fprintf(w, "  funds, and nobody can recover it for you: Hashgram has no administrator,\n")
	fmt.Fprintf(w, "  no password reset and no support channel that can restore an address.\n\n")
	fmt.Fprintf(w, "  Before continuing, make sure that:\n\n")
	fmt.Fprintf(w, "    - this machine is not a Hashgram server\n")
	fmt.Fprintf(w, "    - nothing is recording your screen or terminal scrollback\n")
	fmt.Fprintf(w, "    - you have pen and paper, not a synced password manager\n")
	fmt.Fprintf(w, "    - you will store the paper somewhere that survives a house fire\n\n")
	fmt.Fprintf(w, "  This tool writes nothing to disk. When you close this terminal the\n")
	fmt.Fprintf(w, "  mnemonic is gone.\n\n")
}

func printKey(w interface{ Write([]byte) (int, error) }, mnemonic, address, pubkeyHex, path string, words int) {
	fmt.Fprintf(w, "\n%s\n", strings.Repeat("=", 74))
	fmt.Fprintf(w, "  WRITE THIS DOWN NOW. IT IS SHOWN ONCE.\n")
	fmt.Fprintf(w, "%s\n\n", strings.Repeat("=", 74))

	fmt.Fprintf(w, "  MNEMONIC (%d words)\n\n", words)
	for i, chunk := range chunkWords(mnemonic, 6) {
		fmt.Fprintf(w, "    %2d.  %s\n", i*6+1, chunk)
	}

	fmt.Fprintf(w, "\n%s\n\n", strings.Repeat("-", 74))
	fmt.Fprintf(w, "  PUBLIC ADDRESS  (safe to share; this is what goes in genesis)\n\n")
	fmt.Fprintf(w, "    %s\n\n", address)
	fmt.Fprintf(w, "  Public key      %s\n", pubkeyHex)
	fmt.Fprintf(w, "  Derivation      %s\n", path)
	fmt.Fprintf(w, "\n%s\n\n", strings.Repeat("=", 74))

	fmt.Fprintf(w, "  NEXT STEPS\n\n")
	fmt.Fprintf(w, "    1. Write the mnemonic on paper. Check every word against the screen.\n")
	fmt.Fprintf(w, "    2. Store the paper somewhere safe, and a second copy somewhere else.\n")
	fmt.Fprintf(w, "    3. Verify the backup before you rely on it:\n\n")
	fmt.Fprintf(w, "         hashgram-keygen derive\n\n")
	fmt.Fprintf(w, "       Type in what you wrote down and confirm it produces the SAME ADDRESS\n")
	fmt.Fprintf(w, "       shown above. The address is the check that matters: the BIP-39\n")
	fmt.Fprintf(w, "       checksum is only eight bits and will not catch every transcription\n")
	fmt.Fprintf(w, "       error. A backup you have not tested is not a backup.\n")
	fmt.Fprintf(w, "    4. Close this terminal, and clear its scrollback.\n")
	fmt.Fprintf(w, "    5. Give ONLY the public address to the server:\n\n")
	fmt.Fprintf(w, "         hashgramctl init-mainnet-genesis --founder-address %s\n\n", address)
	fmt.Fprintf(w, "  The mnemonic must never be typed on, copied to, or stored on a Hashgram\n")
	fmt.Fprintf(w, "  server. Nothing about running the network requires it.\n\n")
}

func chunkWords(mnemonic string, per int) []string {
	words := strings.Fields(mnemonic)
	var out []string
	for i := 0; i < len(words); i += per {
		end := i + per
		if end > len(words) {
			end = len(words)
		}
		out = append(out, strings.Join(words[i:end], "  "))
	}
	return out
}

func confirmTyped(expected string) error {
	fmt.Printf("Type %q to continue: ", expected)
	reader := bufio.NewReader(os.Stdin)
	answer, err := reader.ReadString('\n')
	if err != nil {
		return fmt.Errorf("reading confirmation: %w", err)
	}
	if strings.TrimSpace(answer) != expected {
		return fmt.Errorf("aborted")
	}
	return nil
}
