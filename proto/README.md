# Control schemas

`control.proto` defines the controlled-lab control-plane envelope described by
[ADR 0004](../docs/adr/0004-control-plane-schema.md). Rust generation runs in
`sonimesh-protocol` with a vendored `protoc`; generated source is not checked in.

The schema deliberately contains only low-rate hello/capability exchange,
start/stop commands, stream status, and structured errors. It is not a pairing or
transport-security protocol and must run inside the future authenticated control
session.

Start and Stop commands identify media by session ID, stream ID, and an explicit
16-bit-compatible `media_key_epoch`; their rotation and stale-command semantics
are defined by [ADR 0006](../docs/adr/0006-media-key-epoch-control.md).

The version-one length prefix and `Hello` encoding are fixed by the
[control frame test vector](../docs/protocol/control-frame-vectors.md).
