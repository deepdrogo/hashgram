package hgrpc

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"time"
)

// NodeAPIAddr is the loopback address of the P2P node's local API, matching
// the default in node.toml (`api_addr`).
const NodeAPIAddr = "http://127.0.0.1:26672"

// NodeAPI reads the P2P node's local JSON API.
//
// The API is loopback-only and unauthenticated by design: anything that can
// reach it is already on the machine. hashgramctl is the intended reader.
type NodeAPI struct {
	base string
	http *http.Client
}

// NewNodeAPI returns a client for the node API at base (empty for default).
func NewNodeAPI(base string) *NodeAPI {
	if base == "" {
		base = NodeAPIAddr
	}
	return &NodeAPI{base: base, http: &http.Client{Timeout: 5 * time.Second}}
}

// Get fetches a path into out. A connection error means the node is not
// running, which callers report as such rather than as a failure.
func (n *NodeAPI) Get(ctx context.Context, path string, out any) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, n.base+path, nil)
	if err != nil {
		return err
	}
	resp, err := n.http.Do(req)
	if err != nil {
		return fmt.Errorf("hashgram-node API not reachable at %s: %w", n.base, err)
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(io.LimitReader(resp.Body, 8<<20))
	if err != nil {
		return err
	}
	if resp.StatusCode/100 != 2 {
		return fmt.Errorf("hashgram-node API %s: HTTP %d: %s", path, resp.StatusCode, string(body))
	}
	return json.Unmarshal(body, out)
}

// Reachable reports whether the node API answers.
func (n *NodeAPI) Reachable(ctx context.Context) bool {
	var v map[string]any
	return n.Get(ctx, "/v1/health", &v) == nil
}

// NodeStatus is the subset of /v1/status hashgramctl renders.
type NodeStatus struct {
	NetworkID          string         `json:"network_id"`
	ChainID            string         `json:"chain_id"`
	GenesisHash        string         `json:"genesis_hash"`
	Roles              []string       `json:"roles"`
	Swarm              *NodeSwarm     `json:"swarm"`
	AnnouncementsKnown int            `json:"announcements_known"`
	Mailbox            map[string]any `json:"mailbox"`
	Blobs              map[string]any `json:"blobs"`
	Social             map[string]any `json:"social"`
	Safety             map[string]any `json:"safety"`
	UptimeSecs         int64          `json:"uptime_secs"`
	Version            string         `json:"version"`
}

// NodeSwarm mirrors the swarm section.
type NodeSwarm struct {
	PeerID        string   `json:"peer_id"`
	ListenAddrs   []string `json:"listen_addrs"`
	ExternalAddrs []string `json:"external_addrs"`
	Connected     int      `json:"connected"`
	Verified      int      `json:"verified"`
	Known         int      `json:"known"`
	Banned        int      `json:"banned"`
	KadPeers      int      `json:"kad_peers"`
	Reachability  string   `json:"reachability"`
}

// Status fetches /v1/status.
func (n *NodeAPI) Status(ctx context.Context) (*NodeStatus, error) {
	var s NodeStatus
	if err := n.Get(ctx, "/v1/status", &s); err != nil {
		return nil, err
	}
	return &s, nil
}

// NodePeer mirrors one entry of /v1/peers.
type NodePeer struct {
	PeerID        string   `json:"peer_id"`
	Address       string   `json:"address"`
	Verified      bool     `json:"verified"`
	Roles         []string `json:"roles"`
	Operator      string   `json:"operator"`
	Direction     string   `json:"direction"`
	Score         int      `json:"score"`
	ConnectedSecs int64    `json:"connected_secs"`
	Agent         string   `json:"agent"`
}

// Peers fetches /v1/peers.
func (n *NodeAPI) Peers(ctx context.Context) ([]NodePeer, error) {
	var p []NodePeer
	if err := n.Get(ctx, "/v1/peers", &p); err != nil {
		return nil, err
	}
	return p, nil
}

// Rewards fetches /v1/rewards as a loose map; the shape is owned by the node.
func (n *NodeAPI) Rewards(ctx context.Context) (map[string]any, error) {
	var r map[string]any
	if err := n.Get(ctx, "/v1/rewards", &r); err != nil {
		return nil, err
	}
	return r, nil
}
