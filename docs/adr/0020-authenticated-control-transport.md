# ADR 0020: Authenticated control-plane transport boundary

- Status: Provisional
- Date: 2026-08-23

## Context

The desktop control coordinator already gates every envelope behind an
opaque, process-unique transport token ([ADR 0004](./0004-control-plane-schema.md)):
`install_authenticated_control_session` issues one token only for a trusted
READY session, `accept` rejects any other token before envelope validation,
and connection loss retires the token with all stream state. ADR 0017's
allocator consumes that token when issuing fresh media identities. What was
missing was the network-side primitive that earns such a token: the
untrusted TCP framing reader cannot authorize anything
([ADR 0008](./0008-first-pairing-and-identity.md)), and the provider spike
proved the pairing proof exchange over a live pinned TLS channel but left
the result as raw in-process values. Nothing connected "the peer proved the
pairing transcript over this channel" to "this channel may now carry
control envelopes".

## Decision

`sonimesh-network` gains a type-state wrapper around an established pinned
channel, split across two states:

- `AuthenticatedControlHandshake` owns the pre-authentication state. It is
  constructed with a caller-built [`PairingBinding`] (scanned invitation +
  live TLS exporter), the invitation's pairing ID, and the endpoint's local
  role. Its only I/O methods are `read_peer_frame`,
  `verify_peer_frame`, and `send_reply_frame`: exactly one canonical
  [ADR 0019](./0019-pairing-proof-exchange-frame.md) frame carrying this
  invitation's pairing ID and the opposite role byte, verified in constant
  time through the transcript binding before the media root is derived and
  the reply frame rendered.
- `AuthenticatedControlStream` is reachable **only** through
  `into_framing`, which requires a verified exchange. It forwards to the
  bounded [`UntrustedControlStream`] framing unchanged.

Because the framed state is a distinct type, a caller cannot read or write
control envelopes over an unauthenticated channel: the unverified wrapper
has no envelope methods at all. This is defense in depth beneath the
coordinator's token gate — the platform owner still calls
`install_authenticated_control_session` only after `into_framing`
succeeded, and every envelope still carries the issued token.

The wrapper performs no TLS, certificate validation, QR decoding,
pairing-record consumption, attempt accounting, or identity storage; those
remain owned by the platform TLS provider and the application-domain
pairing owner. The media root from `VerifiedPeerProof` feeds the existing
[`MediaSessionAllocator`](./0017-control-reconnect-fresh-media-session.md)
boundary unchanged.

## Security implications

The type-state split makes an unauthenticated control channel unrepresentable:
there is no code path from a constructed handshake to an envelope send or poll
without passing the constant-time proof verification. Role-byte reflection and
foreign invitation frames are rejected at the wire boundary before derivation,
matching the desktop composition's checks. The binding and reply bytes stay
inside the wrapper, `Debug` output redacts them, and a failed verification
leaves the wrapper unusable (`into_framing` refuses), so no half-trusted
channel can outlive a failed pairing. Envelope framing after transition keeps
every ADR 0004 bound (64 KiB frame limit, bounded queue, fail-closed framing
errors).

## Validation evidence

- Unit tests drive both roles over in-memory duplex pipes: a verified
  exchange unlocks `into_framing`, the framed stream forwards a validated
  hello envelope, and the media roots sealed under both sides' derivations
  are byte-identical.
- Rejection shapes are covered at the new boundary: reflected role byte,
  foreign invitation pairing ID, noncanonical frame bytes, wrong proof
  inside a canonical frame, short-exporter construction failure, and
  `send_reply_frame`/`into_framing` refusal on an unverified wrapper.
- The Windows TLS provider spike remains the live-channel evidence for the
  same exchange shape; this module adds the type-state boundary around it.
- No wire-schema change: envelopes and proof frames keep their published
  layouts and cross-language vectors.

## Rollback plan

The module is additive and self-contained. Remove
`src/authenticated_control_stream.rs`, its re-exports, and this record;
the untrusted framing, protocol codec, pairing boundaries, coordinator, and
allocator are unchanged.
