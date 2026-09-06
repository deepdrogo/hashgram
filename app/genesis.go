package app

import "encoding/json"

// GenesisState is the raw application genesis: a map from module name to that
// module's genesis JSON.
//
// The bytes of the assembled genesis file are what the network's genesis_hash
// commits to (see app/params.ComputeGenesisHash), so nothing here may be
// re-serialised on the way to disk.
type GenesisState map[string]json.RawMessage
