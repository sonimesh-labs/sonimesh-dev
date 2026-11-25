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

- The allocator retains the root secret (`SessionSecret`) and returns an
  `IssuedMediaSession` grant that borrows the root together with its derived
  `MediaKeyContext`; callers cannot obtain an allocator root without first
  receiving a grant, reducing accidental root/context mispairing at the call
  boundary.
- The no-reuse key is the full key context: session ID, stream ID, key epoch,
  and direction. A key rotation (same session, higher epoch) and a fresh
  session (new session ID) are both new contexts; replaying any previously
  issued context is rejected.
- The no-reuse record has a fixed bound (`MAX_RETAINED_MEDIA_IDENTITIES`,
  16 entries). When it fills, the allocator fails closed with an explicit
  capacity error and the operator must establish a fresh pairing root.
- A fresh pairing root replaces the whole allocator: drop the old instance and
  construct a new one, which resets the no-reuse record together with the root
  it was derived from. A revocation owner may instead call `revoke` on the
  retained allocator; that drops the root immediately, clears cached contexts,
  and makes every later supply fail closed. Live grants borrow the allocator,
  so revocation cannot race an outstanding grant at the Rust API boundary.
- The allocator performs no TLS, QR, pairing-record consumption, or network
  I/O. The authenticated control-plane transport that selects the media
  identity and the initial root provisioning remain external gates. When both
  application owners are available, `supply_for_authenticated_control_session`
  checks the coordinator's currently installed opaque transport token before
  validating or recording a media identity. A stale token therefore cannot
  consume one of the bounded freshness slots; the lower-level `supply`
  primitive remains available for an already-authenticated external transport
  adapter. The `revoke_for_pairing` composition method is the preferred app
  boundary when the pairing controller is available: it only drops the root
  after the pairing and control owners have completed their terminal
  revocation transition. The reconnect-grace composition delegates the clock
  observation to `ControlStreamCoordinator` and revokes the root only after
  that owner reports an expired window or a fail-closed clock regression; an
  in-grace or idle observation retains the allocator.

An opt-in receiver control path may prepare an existing pipeline for a newly
accepted Start before admission. The host receives the validated request and the
current coordinator and transport token, so it can request a fresh allocator
grant and install it on the existing UDP socket. The preparation path rejects a
new start while another stream is active. The host must preserve pipeline state
on preparation failure, retain or retire the allocator with pairing lifecycle,
and avoid reissuing a context already installed during an earlier failed output
start. A failure aborts the pending start; duplicate commands and rotations do
not invoke this preparation path. This boundary does not construct a platform
host or claim production interoperability.

## Security implications

Rejecting any previously issued key context within a pairing lifetime means a
stale or replayed reconnection cannot re-install a retired session. Explicit
allocator revocation additionally drops the root and clears cached contexts,
so packets sealed under a retired context cannot be re-admitted after a fresh
session replaces it. Reconnect-grace expiry and clock regression use the same
terminal retirement path, preventing a stale pairing root from issuing fresh
contexts after the control session has failed closed. The secret is zeroized on
drop, its `Debug` output is redacted,
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
- Unit coverage verifies that explicit allocator revocation drops the root
  boundary, clears cached contexts, and rejects later supply attempts; the
  pairing-composition path also proves that control and pairing revocation
  complete before the media root is dropped, and a failed pairing transition
  leaves the root and no-reuse record intact. Reconnect-grace unit coverage
  proves that both deadline expiry and a clock regression retire the control
  session and the media root together, while an in-grace observation retains
  it. It also verifies that the coordinator-gated allocator rejects a retired
  control token before consuming no-reuse capacity.
- A control-domain integration test drives the same reconnection through `ControlStreamCoordinator`: connection loss arms a bounded reconnect window while the live UDP path keeps admitting packets, the reconnected transport installs a fresh token and repeats hello negotiation, the retired token fails before it can reserve the reconnection identity, the fresh token supplies a new context for the re-admitted stream, `install_fresh_session` keeps the socket, packets sealed under the retired session are rejected, and the fresh session's packets rebind and decode; the no-reuse record then rejects both issued contexts.
- An opt-in preparation test aborts a failed start and retries it, while a
  duplicate and a competing active start do not invoke preparation. A native
  Opus software integration exercises the callback with a fresh allocator grant,
  local TCP control, protected UDP, and memory output. It does not exercise a
  real pairing transport or platform host; recovery after output-start failure
  remains the host's responsibility.
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
