# ADR 0017: Authenticated control-plane reconnection supplies a fresh media session

- Status: Provisional
- Date: 2026-08-16

## Context

The UDP media receiver pins a source endpoint only after a datagram completes
authentication, replay admission, and a bounded jitter decision. Changing the
pinned endpoint requires a fresh authenticated media session and fresh key
material rather than automatic address migration. The receiver pipeline already
exposes `install_fresh_session` to replace the ingress and replay state with a
new session, and ADR 0006 requires a fresh media-session installation to use a
new `media_session_id`. The authenticated control-plane reconnection that
supplies that fresh session, however, remained unimplemented: nothing retained
the one-time pairing media root, issued fresh `(secret, context)` pairs, or
enforced that a media key context is never re-issued within a pairing lifetime.

First pairing, TLS transcript binding, and the authenticated transport that
selects the media identity remain outside this decision; ADR 0008 and the
platform gates still own them.

## Decision

A new `MediaSessionAllocator` in `sonimesh-app` is the pairing/control-session
owner of the one-time pairing media root. It issues a fresh media session only
for a media identity that the authenticated control plane supplies (for example
from a `StartStream` or `Rotate` command after an authenticated reconnection)
and whose full key context has never been issued before within this pairing
lifetime.

- The allocator retains the root secret (`SessionSecret`) and returns it by
  borrow together with the derived `MediaKeyContext`; it never exposes the
  secret through a public owned value.
- The no-reuse key is the full key context: session ID, stream ID, key epoch,
  and direction. A key rotation (same session, higher epoch) and a fresh
  session (new session ID) are both new contexts; replaying any previously
  issued context is rejected.
- The no-reuse record has a fixed bound (`MAX_RETAINED_MEDIA_IDENTITIES`,
  16 entries). When it fills, the allocator fails closed with an explicit
  capacity error and the operator must establish a fresh pairing root.
- A fresh pairing root replaces the whole allocator: drop the old instance and
  construct a new one, which resets the no-reuse record together with the root
  it was derived from.
- The allocator performs no TLS, QR, pairing-record consumption, or network
  I/O. The authenticated control-plane transport that selects the media
  identity and the initial root provisioning remain external gates.

## Security implications

Rejecting any previously issued key context within a pairing lifetime means a
stale or replayed reconnection cannot re-install a retired session, so packets
sealed under a retired context cannot be re-admitted after a fresh session
replaces it. The secret is zeroized on drop, its `Debug` output is redacted,
and the allocator fails closed when the bounded no-reuse record is full rather
than silently weakening the guarantee.

## Validation evidence

- Unit tests cover a fresh first issue, reuse rejection, zero-identity
  rejection, epoch rotation as a fresh context, new-session freshness,
  direction binding into the no-reuse key, the bounded capacity failure, and
  redacted `Debug`.
- An integration test wires the allocator into `ReceiverStreamPipeline`:
  a packet sealed under the first session is admitted, a fresh reconnection
  session installs through `install_fresh_session`, packets sealed under the
  retired session are then rejected, and the fresh session's packets admit and
  decode. The adaptive-target pipeline test additionally proves the replacement
  is fail-closed for target state: a reused active session ID leaves a populated
  target epoch intact, while a valid fresh session retires it before the first
  new-session packet establishes a fresh arrival baseline.
- A UDP loopback integration test drives the same flow through the real socket
  path: the allocator's retained root backs the sealer and the bind, a fresh
  reconnection session installs over the same socket, packets sealed under the
  retired session are rejected without rebinding the cleared source pin, and
  the fresh session's packets rebind the source and decode.
- The adaptive-target UDP integration test separately proves the receiver's
  state boundary on the same socket: a reused session ID preserves both the
  populated target state and source pin, while a valid fresh session clears
  both before its first authenticated packet re-establishes them.
- A control-domain integration test drives the same reconnection through `ControlStreamCoordinator`: connection loss arms a bounded reconnect window while the live UDP path keeps admitting packets, the reconnected transport installs a fresh token and repeats hello negotiation, the allocator supplies a fresh context for the re-admitted stream, `install_fresh_session` keeps the socket, packets sealed under the retired session are rejected, and the fresh session's packets rebind and decode; the no-reuse record then rejects both issued contexts.
- A seeded property walk drives randomized identity/direction supply scripts
  against an independent append-only set model, asserting the exact outcome
  after every step: no re-issue, no record growth past the bound, no recording
  of a failed supply, and `contains` agreement with the model.
- A byte-driven libFuzzer target (`media_session_allocator` in the `fuzz/`
  workspace) drives the same contract with a deterministic edge prologue and
  fuzz-derived supplies, committed through the finite CI smoke.
- The full `sonimesh-app` test suite passes. No wire schema or cross-language
  vector changes: this is an application-domain component only.

## Rollback plan

The allocator is additive and domain-local. Remove the module and its
re-exports, and revert the baseline and architecture notes; the receiver
pipeline, coordinator, and protocol behavior are unchanged.
