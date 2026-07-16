# Control Frame Test Vector

This vector fixes the version-one control frame length prefix and Protobuf field
encoding for a Kotlin client. It is a wire-format contract only; control frames
must still arrive through the future authenticated control channel.

## Envelope

| Field | Value |
| --- | --- |
| Protocol version | `1` |
| Request ID | `7` |
| Payload | `Hello` |
| Supported versions | `[1]` |
| Capabilities | `[CAPABILITY_FRAME_DURATION_20_MS]` (`2`) |
| Display name UTF-8 | `Lab sender` |

## Expected frame

```text
000000180801100752120a01011201021a0a4c61622073656e646572
```

`00000018` is the unsigned big-endian length of the 24-byte Protobuf payload.
The repeated scalar fields use proto3 packed encoding. Receivers must apply the
same 64 KiB framing limit and semantic validation before treating the envelope
as a command.
