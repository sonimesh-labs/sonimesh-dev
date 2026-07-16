# Control schemas

`control.proto` defines the controlled-lab control-plane envelope described by
[ADR 0004](../docs/adr/0004-control-plane-schema.md). Rust generation runs in
`sonimesh-protocol` with a vendored `protoc`; generated source is not checked in.

The schema deliberately contains only low-rate hello/capability exchange,
start/stop commands, stream status, and structured errors. It is not a pairing or
transport-security protocol and must run inside the future authenticated control
session.
