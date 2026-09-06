package app

import (
	"os"
	"path/filepath"

	"github.com/cosmos/cosmos-sdk/version"

	hgparams "github.com/hashgram/hashgram/app/params"
)

// EnvNodeHome is the environment variable that overrides the node home.
const EnvNodeHome = "HASHGRAM_HOME"

// defaultNodeHome resolves the default node home directory.
//
// Resolution order:
//  1. $HASHGRAM_HOME
//  2. $HOME/.hashgram
//  3. ./.hashgram  (only if $HOME is unset, which happens in some systemd
//     and container setups; failing loudly here would be worse than using a
//     relative path that the operator can see in `hashgramctl status`)
func defaultNodeHome() string {
	if v := os.Getenv(EnvNodeHome); v != "" {
		return v
	}
	home, err := os.UserHomeDir()
	if err != nil || home == "" {
		return hgparams.DefaultNodeHomeDirName
	}
	return filepath.Join(home, hgparams.DefaultNodeHomeDirName)
}

// Version returns the semantic version stamped into the binary.
func Version() string {
	if version.Version != "" {
		return version.Version
	}
	return "dev"
}

// VersionInfo is the machine-readable build identity reported by
// `hashgramctl node-info` and by the P2P handshake.
type VersionInfo struct {
	Version   string `json:"version"`
	Commit    string `json:"commit"`
	BuildDate string `json:"build_date"`
	GoVersion string `json:"go_version"`

	// ProtocolMajorVersion is the wire-compatibility generation. Two nodes
	// with different values here must not exchange application data.
	ProtocolMajorVersion uint32 `json:"protocol_major_version"`
}

// BuildInfo returns the build identity of this binary.
func BuildInfo() VersionInfo {
	return VersionInfo{
		Version:              Version(),
		Commit:               version.Commit,
		BuildDate:            BuildDate,
		GoVersion:            version.NewInfo().GoVersion,
		ProtocolMajorVersion: hgparams.ProtocolMajorVersion,
	}
}
