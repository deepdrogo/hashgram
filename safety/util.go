package safety

import "google.golang.org/protobuf/proto"

func unmarshal(raw []byte, m proto.Message) bool {
	return proto.Unmarshal(raw, m) == nil
}
