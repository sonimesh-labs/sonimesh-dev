# Control Frame Test Vectors

These vectors fix the version-one control frame length prefix and Protobuf field
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

## Epoch-bearing StartStream

| Field | Value |
| --- | --- |
| Protocol version | `1` |
| Request ID | `8` |
| Media session ID | `7` |
| Stream ID | `9` |
| Sample rate | `48000` Hz |
| Channels | `2` |
| Frame duration | `20` ms |
| Media key epoch | `0x2345` (`9029`) |

```text
00000015080110085a0f080710091880f7022002281430c546
```

`media_key_epoch` is a `uint32` on the Protobuf wire but must not exceed
`65535`. Start and Stop commands bind this exact epoch to the media session and
stream identity; see [ADR 0006](../adr/0006-media-key-epoch-control.md).

## Epoch-bearing StopStream

| Field | Value |
| --- | --- |
| Protocol version | `1` |
| Request ID | `9` |
| Media session ID | `7` |
| Stream ID | `9` |
| Media key epoch | `0x2345` (`9029`) |

```text
0000000d0801100962070807100918c546
```

`0000000d` declares the 13-byte Protobuf payload. The exact session, stream, and
epoch tuple prevents a delayed stop from targeting a newer stream generation.

## StreamStatus

| Field | Value |
| --- | --- |
| Protocol version | `1` |
| Request ID | `10` |
| Media session ID | `7` |
| Stream ID | `9` |
| State | `STREAM_STATE_STREAMING` (`2`) |
| Detail UTF-8 | `receiving` |

```text
000000170801100a6a110807100918022209726563656976696e67
```

`00000017` declares the 23-byte Protobuf payload. Version one requires nonzero
media/stream identities and a known, non-unspecified state.

## ControlError

| Field | Value |
| --- | --- |
| Protocol version | `1` |
| Request ID | `11` |
| Code | `ERROR_CODE_RETRY_LATER` (`5`) |
| Retryable | `true` |
| Detail UTF-8 | `try again` |

```text
000000150801100b720f080510011a0974727920616761696e
```

`00000015` declares the 21-byte Protobuf payload. Version one requires a known,
non-unspecified structured error code.

## Cross-language verification

Rust protocol tests and the Android host-JVM `ControlFrameVectorTest`
independently encode all five published payload shapes and decode their semantic
fields. The Kotlin test also rejects truncated, mismatched, and oversized length
prefixes, a `media_key_epoch` above `65535`, zero status identities, and
unspecified or unknown status/error enums.

The Android host-JVM `HelloNegotiation` mirror, exercised by
`HelloNegotiationVectorTest` and by `ControlFrameVectorTest`'s wire-decode
tests, pins `negotiate_hello`'s canonical capability intersection and the
bounded hello admission rules (16 capabilities, 8 supported versions, 128-byte
display name) so the sender and receiver select the same version-one control
capability set.

This is wire-format and host-test evidence only. The Android production source
set still contains no control transport, generated Protobuf client, pairing, or
TLS session.
