// Package hgrpc talks to a local Hashgram node over CometBFT RPC.
//
// Deliberately a small hand-rolled HTTP client rather than the CometBFT RPC
// client library. hashgramctl runs on the same machine as the node and only
// needs a handful of read-only endpoints; pulling in the full RPC client
// would tie the operator tool's build to the consensus engine's version, so
// an operator could not use a newer hashgramctl against an older node.
package hgrpc

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"strconv"
	"time"
)

// DefaultEndpoint is the loopback CometBFT RPC address.
//
// Loopback, because the RPC surface includes administrative endpoints. Public
// read access is a deliberate reverse-proxy decision, not a default.
const DefaultEndpoint = "http://127.0.0.1:26657"

// Client is a minimal CometBFT RPC client.
type Client struct {
	Endpoint string
	HTTP     *http.Client
}

// New returns a client with a short timeout.
//
// Short, because every caller is an interactive operator command: a node that
// is not answering in five seconds is a node the operator needs to be told
// about, not one worth waiting on.
func New(endpoint string) *Client {
	if endpoint == "" {
		endpoint = DefaultEndpoint
	}
	return &Client{
		Endpoint: endpoint,
		HTTP:     &http.Client{Timeout: 5 * time.Second},
	}
}

// Status is the subset of CometBFT's /status that operators need.
type Status struct {
	NodeInfo struct {
		ID      string `json:"id"`
		Network string `json:"network"`
		Moniker string `json:"moniker"`
		Version string `json:"version"`
		Other   struct {
			TxIndex   string `json:"tx_index"`
			RPCAddres string `json:"rpc_address"`
		} `json:"other"`
	} `json:"node_info"`

	SyncInfo struct {
		LatestBlockHash   string `json:"latest_block_hash"`
		LatestAppHash     string `json:"latest_app_hash"`
		LatestBlockHeight string `json:"latest_block_height"`
		LatestBlockTime   string `json:"latest_block_time"`
		EarliestHeight    string `json:"earliest_block_height"`
		CatchingUp        bool   `json:"catching_up"`
	} `json:"sync_info"`

	ValidatorInfo struct {
		Address     string `json:"address"`
		VotingPower string `json:"voting_power"`
		PubKey      struct {
			Type  string `json:"type"`
			Value string `json:"value"`
		} `json:"pub_key"`
	} `json:"validator_info"`
}

// Height returns the latest block height as an integer.
func (s Status) Height() int64 {
	n, _ := strconv.ParseInt(s.SyncInfo.LatestBlockHeight, 10, 64)
	return n
}

// VotingPower returns the validator voting power as an integer.
func (s Status) VotingPower() int64 {
	n, _ := strconv.ParseInt(s.ValidatorInfo.VotingPower, 10, 64)
	return n
}

// IsValidator reports whether this node is in the active set.
func (s Status) IsValidator() bool { return s.VotingPower() > 0 }

// Status fetches /status.
func (c *Client) Status(ctx context.Context) (*Status, error) {
	var out struct {
		Result Status `json:"result"`
	}
	if err := c.get(ctx, "/status", &out); err != nil {
		return nil, err
	}
	return &out.Result, nil
}

// Peer is one connected peer.
type Peer struct {
	NodeInfo struct {
		ID      string `json:"id"`
		Moniker string `json:"moniker"`
		Network string `json:"network"`
		Version string `json:"version"`
	} `json:"node_info"`
	IsOutbound bool   `json:"is_outbound"`
	RemoteIP   string `json:"remote_ip"`
}

// NetInfo is the subset of CometBFT's /net_info that operators need.
type NetInfo struct {
	Listening bool     `json:"listening"`
	Listeners []string `json:"listeners"`
	NPeers    string   `json:"n_peers"`
	Peers     []Peer   `json:"peers"`
}

// PeerCount returns the peer count as an integer.
func (n NetInfo) PeerCount() int {
	v, _ := strconv.Atoi(n.NPeers)
	return v
}

// NetInfo fetches /net_info.
func (c *Client) NetInfo(ctx context.Context) (*NetInfo, error) {
	var out struct {
		Result NetInfo `json:"result"`
	}
	if err := c.get(ctx, "/net_info", &out); err != nil {
		return nil, err
	}
	return &out.Result, nil
}

// GenesisChunked fetches the genesis document.
//
// CometBFT serves genesis in chunks because a large genesis exceeds its
// response limit. Reassembling here means a client can hash the genesis it
// was actually served and compare it with the published hash, which is the
// only way to verify the network identity without trusting the node.
func (c *Client) GenesisChunked(ctx context.Context) ([]byte, error) {
	var first struct {
		Result struct {
			Chunk string `json:"chunk"`
			Total string `json:"total"`
			Data  []byte `json:"data"`
		} `json:"result"`
	}
	if err := c.get(ctx, "/genesis_chunked?chunk=0", &first); err != nil {
		return nil, err
	}

	total, err := strconv.Atoi(first.Result.Total)
	if err != nil {
		return nil, fmt.Errorf("unexpected genesis chunk total %q: %w", first.Result.Total, err)
	}

	out := append([]byte(nil), first.Result.Data...)
	for i := 1; i < total; i++ {
		var next struct {
			Result struct {
				Data []byte `json:"data"`
			} `json:"result"`
		}
		if err := c.get(ctx, fmt.Sprintf("/genesis_chunked?chunk=%d", i), &next); err != nil {
			return nil, err
		}
		out = append(out, next.Result.Data...)
	}
	return out, nil
}

// ABCIInfo is the subset of /abci_info operators need.
type ABCIInfo struct {
	Data             string `json:"data"`
	Version          string `json:"version"`
	AppVersion       string `json:"app_version"`
	LastBlockHeight  string `json:"last_block_height"`
	LastBlockAppHash []byte `json:"last_block_app_hash"`
}

// ABCIInfo fetches /abci_info.
func (c *Client) ABCIInfo(ctx context.Context) (*ABCIInfo, error) {
	var out struct {
		Result struct {
			Response ABCIInfo `json:"response"`
		} `json:"result"`
	}
	if err := c.get(ctx, "/abci_info", &out); err != nil {
		return nil, err
	}
	return &out.Result.Response, nil
}

// Reachable reports whether the node's RPC is answering.
func (c *Client) Reachable(ctx context.Context) bool {
	_, err := c.Status(ctx)
	return err == nil
}

func (c *Client) get(ctx context.Context, path string, out any) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, c.Endpoint+path, nil)
	if err != nil {
		return err
	}
	resp, err := c.HTTP.Do(req)
	if err != nil {
		return fmt.Errorf("the node RPC at %s is not answering: %w", c.Endpoint, err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("%s%s returned HTTP %d", c.Endpoint, path, resp.StatusCode)
	}
	return json.NewDecoder(resp.Body).Decode(out)
}
