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

An isolated paired-audio lab composes this boundary with the existing UDP
receiver: the desktop prepares the receiver from the pairing root and the
authenticated StartStream identity before acknowledging Streaming, and retires
it before acknowledging StopStream. Its fixed invitation and local ports are
test fixtures, not a persistent trust or production discovery mechanism. By
default the control and media listeners bind to loopback; explicit IPv4 bind and
invitation-host arguments permit a manually addressed controlled-network lab.
Both peers must use the same invitation host because it is transcript-bound.
The public fixed invitation does not establish production device trust; do not
expose this experiment outside a controlled lab network. The optional
continuous-listener mode handles one client at a time and starts a
fresh invitation owner, control coordinator, and media session for each accepted
connection. The process keeps its temporary TLS identity until it exits; a
finished or failed session releases its output and returns to listening. This
is a lab convenience, not persistent pairing or automatic reconnection.
The lab also offers an optional Windows CPAL output: it starts the output before
acknowledging Streaming and stops it before acknowledging Stopped. If the output
reports a fault during a session, the lab keeps the control and media session
alive and retries the selected output once per second. It suspends playout until
the output is recreated with a fresh PCM queue, then waits for new media before
resuming. Its software checks and callback counts do not
establish audible playback or latency on hardware.
With explicit console control enabled, local volume and mute changes use the
output's existing gain handle and persist across sequential sessions and output
recovery. They do not alter the control-plane wire protocol.
Its fixed 20 ms lab playout schedule advances from the previous deadline to
avoid accumulating ordinary polling delay; if a whole period is missed, it
rebases from the current time rather than issuing a burst of catch-up decodes.
This is a lab scheduling policy, not a hardware-qualified drift controller.
Malformed, oversized, replayed, unauthenticated, wrong-stream, or wrong-source
UDP datagrams are rejected individually by this lab while socket and internal
failures still terminate it. Authenticated frames rejected by the bounded frame
admission are likewise discarded without restarting the session.
The Android audio-lab service can also use that same test fixture with
playback capture: it establishes pairing and starts a stream before sending,
then sends StopStream when capture ends. During capture, the same worker polls
the otherwise idle control connection and stops with a connection-ended status
when it closes; this poll and StopStream never read the TLS stream concurrently.
This detects an observed close or read failure, not a silent network partition.
On normal lab shutdown the UDP sender gets up to 2.5 seconds to drain and a
further 0.5 seconds to exit after cancellation. StopStream is attempted only
after it exits, with a 2-second reply timeout; the control socket close grace is
6 seconds and the service waits up to 6.5 seconds for its supervisor. A sender
that does not exit is reported as incomplete, and the connection is closed
without claiming desktop stop confirmation. These are bounded lab policies, not
delivery guarantees.
The user supplies the lab's public
desktop pin manually; this is not QR scanning or persistent device pairing.

The application-domain invitation listener exposes two explicit boundaries:
`PairingListener` owns a non-blocking `TcpListener` and lets a caller poll one
queued peer with `try_accept`, while `accept_pairing_stream` performs the
invitation-gated exchange on an already accepted stream. The split lets a
caller own polling and scheduling without making the mutable pairing owner or
session state concurrent. It is an admission boundary, not a concurrent
pairing server loop. The exchange uses one absolute monotonic deadline, starting
after accepting a TCP peer, for the TLS handshake, proof frame and reply write.
The default is 30 seconds. Each underlying socket operation uses the remaining
time, so intermittent progress does not restart the budget. Blocking accept
itself remains outside this deadline. The checks cannot preempt CPU work or OS
scheduling and therefore do not provide a hard real-time bound. A failed reply
after proof admission leaves the session trusted but returns no usable channel,
as before. Proof admission must account for monotonic time elapsed since the
caller's immediately preceding monotonic time sample,
including a blocking accept, and reject an invitation that expires during the
exchange. Elapsed time is measured with `Instant` and truncated to milliseconds;
the supplied clock must advance consistently with that clock. This estimate can
lag the caller's millisecond clock by less than one millisecond. The blocking
`accept_pairing_connection` compatibility helper remains available for one
connection per call.

The trusted pairing result retains its blocking socket for callers that need
it. The receiver scheduler and `DesktopControlSession::try_from_trusted_channel`
instead configure the socket as non-blocking before polling; failure is
returned before the channel is attached. The existing direct constructor requires
the caller to configure the socket before using it in a polling loop. Bounded framing also waits for the
underlying stream's flush after accepting a frame, retrying transient backpressure
before accepting another reply. This allows a TLS transport to finish writing
buffered ciphertext even when no further command arrives. Completion means the
transport accepted the bytes, not that the peer processed them.

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
- The `sonimesh-app` loopback integration test
  (`tests/authenticated_control_transport.rs`) drives the full application
  path over a real TCP socket pair: proof exchange, framing unlock, hello
  negotiation through `ControlStreamCoordinator`, the installed transport
  token authorizing a fresh identity via the ADR 0017 allocator, and a
  completed start into Streaming.
- The Windows TLS provider spike remains the live-channel evidence for the
  same exchange shape; this module adds the type-state boundary around it.
- The invitation listener's timeout test keeps a peer that sends no ClientHello
  from occupying the pairing owner indefinitely; socket timeouts also apply to
  the fixed proof-frame read and reply write.
- A second timeout test completes the pinned TLS handshake, then withholds the
  proof frame; the socket timeout expires during the proof read before invitation
  admission or session mutation.
- The paired-audio lab's simulated output-fault test exercises a failed retry,
  rate-limited recovery, stale jitter retirement, and fresh authenticated UDP
  audio rendered through the recreated source. Physical device removal and
  audible playback after recovery remain unverified.
- Loopback tests trickle an incomplete TLS record and, separately, proof bytes
  after a completed TLS handshake. The receiver times out while the peer is
  attempting periodic writes, so progress cannot reset the exchange deadline.
- A pinned TLS loopback test sends a valid proof after invitation expiry and
  checks that admission fails, the invitation expires, and the session is idle.
- The non-blocking listener unit test verifies an empty poll followed by
  loopback admission, while the stream-level timeout test verifies that the
  accepted-stream boundary preserves the serialized handshake owner.
- No wire-schema change: envelopes and proof frames keep their published
  layouts and cross-language vectors.

## Rollback plan

The module is additive and self-contained. Remove
`src/authenticated_control_stream.rs`, its re-exports, and this record;
the untrusted framing, protocol codec, pairing boundaries, coordinator, and
allocator are unchanged.
