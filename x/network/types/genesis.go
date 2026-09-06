package types

import (
	"bytes"
	"fmt"

	hgparams "github.com/hashgram/hashgram/app/params"
)

// DefaultGenesis returns the x/network genesis for a development network.
//
// It deliberately returns the DEVNET identity, not Mainnet. `hashgramd init`
// calls this, and a chain accidentally started from defaults must not be able
// to claim to be Hashgram Mainnet. Mainnet genesis is produced only by
// `hashgramctl init-mainnet-genesis`, which calls MainnetGenesis below after
// an explicit operator confirmation.
func DefaultGenesis() *GenesisState {
	return devnetGenesis()
}

func devnetGenesis() *GenesisState {
	id := hgparams.DevnetIdentity("")
	return &GenesisState{
		Info: NetworkInfo{
			NetworkName:          id.NetworkName,
			NetworkId:            id.NetworkID,
			ChainId:              id.ChainID,
			NetworkMagic:         id.NetworkMagic[:],
			ProtocolMajorVersion: id.ProtocolMajorVersion,
		},
		ForkIsolation: StrictForkIsolation(),
	}
}

// MainnetGenesis returns the x/network genesis for Hashgram Mainnet.
func MainnetGenesis() *GenesisState {
	id := hgparams.MainnetIdentity("")
	return &GenesisState{
		Info: NetworkInfo{
			NetworkName:          id.NetworkName,
			NetworkId:            id.NetworkID,
			ChainId:              id.ChainID,
			NetworkMagic:         id.NetworkMagic[:],
			ProtocolMajorVersion: id.ProtocolMajorVersion,
		},
		ForkIsolation: StrictForkIsolation(),
	}
}

// StrictForkIsolation enables every peer-rejection check. This is the only
// configuration Mainnet accepts; Validate rejects anything weaker.
func StrictForkIsolation() ForkIsolationPolicy {
	return ForkIsolationPolicy{
		RequireNetworkIdMatch:     true,
		RequireChainIdMatch:       true,
		RequireNetworkMagicMatch:  true,
		RequireGenesisHashMatch:   true,
		RequireProtocolMajorMatch: true,
	}
}

// Validate checks the genesis state.
func (gs GenesisState) Validate() error {
	if err := gs.Info.Validate(); err != nil {
		return err
	}
	return gs.ForkIsolation.Validate(gs.Info.NetworkId)
}

// Validate checks a NetworkInfo for internal consistency.
func (n NetworkInfo) Validate() error {
	if n.NetworkName == "" {
		return ErrInvalidNetworkInfo.Wrap("network_name must not be empty")
	}
	if n.NetworkId == "" {
		return ErrInvalidNetworkInfo.Wrap("network_id must not be empty")
	}
	if n.ChainId == "" {
		return ErrInvalidNetworkInfo.Wrap("chain_id must not be empty")
	}
	if len(n.NetworkMagic) != 4 {
		return ErrInvalidNetworkInfo.Wrapf("network_magic must be exactly 4 bytes, got %d", len(n.NetworkMagic))
	}
	if bytes.Equal(n.NetworkMagic, []byte{0, 0, 0, 0}) {
		return ErrInvalidNetworkInfo.Wrap("network_magic must not be all zero")
	}
	if n.ProtocolMajorVersion == 0 {
		return ErrInvalidNetworkInfo.Wrap("protocol_major_version must be >= 1")
	}

	// A genesis that claims the Mainnet network_id must carry every other
	// Mainnet identifier. Without this, someone could publish a genesis with
	// network_id "hashgram-mainnet" but their own chain_id or magic and
	// present it to users as Mainnet.
	if n.NetworkId == hgparams.NetworkIDMainnet {
		if n.ChainId != hgparams.ChainIDMainnet {
			return ErrInvalidNetworkInfo.Wrapf(
				"network_id %q requires chain_id %q, got %q",
				hgparams.NetworkIDMainnet, hgparams.ChainIDMainnet, n.ChainId)
		}
		if !bytes.Equal(n.NetworkMagic, hgparams.NetworkMagicMainnet[:]) {
			return ErrInvalidNetworkInfo.Wrapf(
				"network_id %q requires network_magic %q, got %q",
				hgparams.NetworkIDMainnet,
				string(hgparams.NetworkMagicMainnet[:]), string(n.NetworkMagic))
		}
		if n.ProtocolMajorVersion != hgparams.ProtocolMajorVersion {
			return ErrInvalidNetworkInfo.Wrapf(
				"network_id %q requires protocol_major_version %d, got %d",
				hgparams.NetworkIDMainnet, hgparams.ProtocolMajorVersion, n.ProtocolMajorVersion)
		}
	}

	return nil
}

// Validate checks a fork-isolation policy.
//
// On Mainnet every check must be enabled. A "Mainnet" that accepts peers with
// a foreign genesis hash is not fork-isolated, and quietly shipping such a
// genesis would be an effective way to merge a fork into Mainnet.
func (p ForkIsolationPolicy) Validate(networkID string) error {
	if networkID != hgparams.NetworkIDMainnet {
		return nil
	}
	missing := make([]string, 0, 5)
	if !p.RequireNetworkIdMatch {
		missing = append(missing, "require_network_id_match")
	}
	if !p.RequireChainIdMatch {
		missing = append(missing, "require_chain_id_match")
	}
	if !p.RequireNetworkMagicMatch {
		missing = append(missing, "require_network_magic_match")
	}
	if !p.RequireGenesisHashMatch {
		missing = append(missing, "require_genesis_hash_match")
	}
	if !p.RequireProtocolMajorMatch {
		missing = append(missing, "require_protocol_major_match")
	}
	if len(missing) > 0 {
		return ErrForkIsolationWeakened.Wrapf("disabled on mainnet: %v", missing)
	}
	return nil
}

// Identity converts on-chain NetworkInfo into the shared identity type, with
// the supplied genesis hash pinned.
//
// The genesis hash is a parameter rather than a stored field because the hash
// of the genesis file cannot be a value inside that same file. It comes from
// node configuration, and any client can recompute it from the genesis
// document served over CometBFT RPC.
func (n NetworkInfo) Identity(genesisHash string) hgparams.NetworkIdentity {
	var magic [4]byte
	copy(magic[:], n.NetworkMagic)

	return hgparams.NetworkIdentity{
		NetworkName:          n.NetworkName,
		NetworkID:            n.NetworkId,
		ChainID:              n.ChainId,
		NetworkMagic:         magic,
		ProtocolMajorVersion: n.ProtocolMajorVersion,
		GenesisHash:          genesisHash,
	}
}

// Describe renders a NetworkInfo as a single line for operator output.
//
// Named Describe rather than String because gogoproto already generates a
// String method on the generated type.
func (n NetworkInfo) Describe() string {
	return fmt.Sprintf(
		"%s (network_id=%s chain_id=%s magic=%s protocol=v%d)",
		n.NetworkName, n.NetworkId, n.ChainId, string(n.NetworkMagic), n.ProtocolMajorVersion,
	)
}
