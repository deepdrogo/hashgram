// Package hgconfig reads and writes the operator-facing Hashgram
// configuration that lives outside the chain's own node home.
//
// Two files matter:
//
//	/etc/hashgram/network.json  the pinned network identity, including the
//	                            genesis hash. Written once at genesis or when
//	                            joining, and read by every command that needs
//	                            to answer "which network is this?"
//
//	/etc/hashgram/roles.json    which node roles this machine serves.
//
// They are deliberately separate from the node home. The node home is chain
// data that can be wiped and re-synced; these files are operator intent that
// should survive that.
package hgconfig

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	hgparams "github.com/hashgram/hashgram/app/params"
)

// DefaultConfigDir is where operator configuration lives.
const DefaultConfigDir = "/etc/hashgram"

// DefaultDataDir is where node state lives.
const DefaultDataDir = "/var/lib/hashgram"

// File names.
const (
	NetworkFileName = "network.json"
	RolesFileName   = "roles.json"
)

// Role is a node role this machine may serve.
type Role string

// The node roles from docs/NODE_ROLES.md.
const (
	RoleValidator Role = "validator"
	RoleRelay     Role = "relay"
	RoleStore     Role = "store"
	RoleMedia     Role = "media"
	RoleIndexer   Role = "indexer"
	RoleBootstrap Role = "bootstrap"
	RoleCall      Role = "call"
	RoleSafety    Role = "safety"
)

// AllRoles returns every valid role, in a stable order.
func AllRoles() []Role {
	return []Role{
		RoleValidator, RoleRelay, RoleStore, RoleMedia,
		RoleIndexer, RoleBootstrap, RoleCall, RoleSafety,
	}
}

// ParseRoles parses a comma-separated role list.
//
// Unknown roles are an error rather than a warning: an operator who typed
// "vaildator" intended to run a validator, and silently running nothing would
// be the worst outcome.
func ParseRoles(s string) ([]Role, error) {
	valid := make(map[Role]bool, len(AllRoles()))
	for _, r := range AllRoles() {
		valid[r] = true
	}

	seen := make(map[Role]bool)
	var out []Role
	for _, part := range strings.Split(s, ",") {
		name := Role(strings.ToLower(strings.TrimSpace(part)))
		if name == "" {
			continue
		}
		if !valid[name] {
			return nil, fmt.Errorf("unknown role %q; valid roles: %s", name, RoleNames())
		}
		if seen[name] {
			continue
		}
		seen[name] = true
		out = append(out, name)
	}
	if len(out) == 0 {
		return nil, errors.New("at least one role is required")
	}
	sort.Slice(out, func(i, j int) bool { return out[i] < out[j] })
	return out, nil
}

// RoleNames returns the valid role names as a comma-separated string.
func RoleNames() string {
	names := make([]string, 0, len(AllRoles()))
	for _, r := range AllRoles() {
		names = append(names, string(r))
	}
	return strings.Join(names, ", ")
}

// Network is the pinned network identity.
type Network struct {
	NetworkName          string `json:"network_name"`
	NetworkID            string `json:"network_id"`
	ChainID              string `json:"chain_id"`
	NetworkMagic         string `json:"network_magic"`
	ProtocolMajorVersion uint32 `json:"protocol_major_version"`

	// GenesisHash is the lowercase hex SHA-256 of the canonical genesis file.
	// This is the value that makes "is this really Hashgram Mainnet?" a
	// checkable question rather than a claim.
	GenesisHash string `json:"genesis_hash"`

	// GenesisTime and CreatedBy are provenance, not identity. Recorded so an
	// operator can tell which run produced this file.
	GenesisTime string `json:"genesis_time,omitempty"`
	CreatedBy   string `json:"created_by,omitempty"`
}

// Identity converts the pinned configuration into the shared identity type.
func (n Network) Identity() hgparams.NetworkIdentity {
	var magic [4]byte
	copy(magic[:], n.NetworkMagic)
	return hgparams.NetworkIdentity{
		NetworkName:          n.NetworkName,
		NetworkID:            n.NetworkID,
		ChainID:              n.ChainID,
		NetworkMagic:         magic,
		ProtocolMajorVersion: n.ProtocolMajorVersion,
		GenesisHash:          n.GenesisHash,
	}
}

// Validate checks the pinned configuration.
func (n Network) Validate() error {
	if err := n.Identity().Validate(); err != nil {
		return err
	}
	if n.GenesisHash == "" {
		return errors.New("genesis_hash is not pinned; this node cannot verify which network it is on")
	}
	return hgparams.ValidateGenesisHash(n.GenesisHash)
}

// IsMainnet reports whether this configuration pins Hashgram Mainnet.
func (n Network) IsMainnet() bool { return n.Identity().IsMainnet() }

// Roles is the role configuration.
type Roles struct {
	Roles []Role `json:"roles"`

	// RewardAddress receives useful-service rewards. It should differ from
	// any key held on this machine, so that a compromised node cannot move
	// its own earnings.
	RewardAddress string `json:"reward_address,omitempty"`

	// DeclaredStorageBytes and DeclaredBandwidthBps are what this node
	// advertises. Advertising, not entitlement: payment follows what is
	// actually assigned and verified.
	DeclaredStorageBytes uint64 `json:"declared_storage_bytes,omitempty"`
	DeclaredBandwidthBps uint64 `json:"declared_bandwidth_bps,omitempty"`

	Moniker string `json:"moniker,omitempty"`
}

// Has reports whether a role is configured.
func (r Roles) Has(role Role) bool {
	for _, x := range r.Roles {
		if x == role {
			return true
		}
	}
	return false
}

// String renders the role list.
func (r Roles) String() string {
	names := make([]string, 0, len(r.Roles))
	for _, x := range r.Roles {
		names = append(names, string(x))
	}
	return strings.Join(names, ",")
}

// Paths resolves the configuration file locations.
type Paths struct {
	ConfigDir string
	DataDir   string
	NodeHome  string
}

// NetworkFile returns the network configuration path.
func (p Paths) NetworkFile() string { return filepath.Join(p.ConfigDir, NetworkFileName) }

// RolesFile returns the role configuration path.
func (p Paths) RolesFile() string { return filepath.Join(p.ConfigDir, RolesFileName) }

// GenesisFile returns the node's genesis file path.
func (p Paths) GenesisFile() string { return filepath.Join(p.NodeHome, "config", "genesis.json") }

// ValidatorKeyFile returns the consensus signing key path.
//
// This is the single most sensitive file on a validator. Its permissions are
// checked by `hashgramctl mainnet-preflight`, and docs/SECURITY.md covers
// moving it off the machine entirely behind a remote signer.
func (p Paths) ValidatorKeyFile() string {
	return filepath.Join(p.NodeHome, "config", "priv_validator_key.json")
}

// NodeKeyFile returns the P2P identity key path.
func (p Paths) NodeKeyFile() string { return filepath.Join(p.NodeHome, "config", "node_key.json") }

// AppConfigFile returns the application config path.
func (p Paths) AppConfigFile() string { return filepath.Join(p.NodeHome, "config", "app.toml") }

// CometConfigFile returns the CometBFT config path.
func (p Paths) CometConfigFile() string { return filepath.Join(p.NodeHome, "config", "config.toml") }

// LoadNetwork reads the pinned network configuration.
func LoadNetwork(p Paths) (Network, error) {
	raw, err := os.ReadFile(p.NetworkFile())
	if err != nil {
		if os.IsNotExist(err) {
			return Network{}, fmt.Errorf(
				"no network configuration at %s; run 'hashgramctl init-mainnet-genesis' to create a "+
					"network or 'hashgramctl join-mainnet' to join one", p.NetworkFile())
		}
		return Network{}, err
	}
	var n Network
	if err := json.Unmarshal(raw, &n); err != nil {
		return Network{}, fmt.Errorf("parsing %s: %w", p.NetworkFile(), err)
	}
	return n, nil
}

// SaveNetwork writes the pinned network configuration.
//
// Refuses to overwrite an existing file. Silently replacing a pinned identity
// is how a node ends up on a different network than its operator believes,
// which is precisely the failure this file exists to prevent.
func SaveNetwork(p Paths, n Network, force bool) error {
	if err := n.Validate(); err != nil {
		return fmt.Errorf("refusing to write an invalid network configuration: %w", err)
	}
	if !force {
		if _, err := os.Stat(p.NetworkFile()); err == nil {
			return fmt.Errorf(
				"%s already exists; refusing to silently repin this node's network identity. "+
					"Move the existing file aside if you really mean to change networks",
				p.NetworkFile())
		}
	}
	return writeJSON(p.NetworkFile(), n, 0o644)
}

// LoadRoles reads the role configuration, returning an empty set if absent.
func LoadRoles(p Paths) (Roles, error) {
	raw, err := os.ReadFile(p.RolesFile())
	if err != nil {
		if os.IsNotExist(err) {
			return Roles{}, nil
		}
		return Roles{}, err
	}
	var r Roles
	if err := json.Unmarshal(raw, &r); err != nil {
		return Roles{}, fmt.Errorf("parsing %s: %w", p.RolesFile(), err)
	}
	return r, nil
}

// SaveRoles writes the role configuration.
func SaveRoles(p Paths, r Roles) error {
	return writeJSON(p.RolesFile(), r, 0o644)
}

func writeJSON(path string, v any, mode os.FileMode) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	raw, err := json.MarshalIndent(v, "", "  ")
	if err != nil {
		return err
	}
	raw = append(raw, '\n')

	// Written to a temporary file and renamed, so a crash mid-write cannot
	// leave a half-written identity file behind.
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, raw, mode); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}
