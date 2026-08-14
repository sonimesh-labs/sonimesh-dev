# SoniMesh fuzz targets

This independent Cargo workspace contains the LibFuzzer entry points for bounded
untrusted protocol and media ingress. It is intentionally separate from the
production workspace so `libfuzzer-sys` never becomes a shipping dependency.

The network-scenario targets (`jitter_target`, `jitter_scheduler`, `multi_stream`, `multi_stream_jitter_target`, `mixer_membership_scenario`, `mixer_jitter_scheduler`, `mixer_jitter_scheduler_4s`, and `mixer_controller_membership`) derive their virtual-link profiles from input bytes, including a serial-link bandwidth, an in-flight bound, and a bounded sender-side socket buffer, so arbitrary inputs also exercise finite-link serialization and backpressure alongside loss, reorder, and duplication.

## Targets

- `adaptive_jitter_target` feeds arbitrary arrival ticks and sender sample
  timestamps into the relative arrival-jitter estimator and then the adaptive
  target controller with arbitrary buffered-frame counts, proving adversarial
  timing cannot panic the controller, move the target outside its bounded
  `[min, max]` water levels, or corrupt the reset path.
- `control_stream_coordinator` drives the real pairing path to a trusted ready session, installs one authenticated transport, and feeds arbitrary decoded control frames plus mutations of the published Hello, StartStream, and StopStream frames through the coordinator, committing or aborting every produced Start, Rotate, and Stop action with its token. It proves authenticated adversarial bytes cannot panic the coordinator, admit media past `MAX_ACTIVE_STREAMS`, or complete work across a session-expiry or reconnect boundary.
- `control_to_mixer` establishes one trusted, negotiated control session and then drives arbitrary start, rotation, stop, output-fault recovery, reconnect-grace, and session-expiry scripts through the bridge composition (ADR 0015), committing or aborting every produced action. It proves adversarial lifecycle interleavings cannot panic the bridge, admit media past the fixed slot bound, or let the snapshot claim live streams or an active output that the coordinator no longer tracks.
- `control_frame` feeds arbitrary bytes plus mutations of the five published
  Hello, StartStream, StopStream, StreamStatus, and ControlError frames into the
  length-prefixed Protobuf control decoder.
- `media_packet` feeds arbitrary and near-valid bytes into the bounded media
  packet parser.
- `nominal_resampler` feeds arbitrary sender-declared input and output sample
  rates, channel counts, drift corrections, and buffer lengths into the bounded
  linear resampler, proving adversarial rate metadata cannot panic the
  interpolation loop, exceed the consumed/produced bounds, or leave output
  uninitialized on a rejected step.
- `channel_mapper` feeds arbitrary negotiated input/output channel layouts,
  buffer lengths, and finite sample payloads into the bounded interleaved
  mapper, proving layout metadata cannot panic the mapping loop, inflate the
  produced count, or leave output uninitialized on a rejected step.
- `peak_limiter` feeds arbitrary validated ceilings and release coefficients
  plus arbitrary interleaved sample payloads, including non-finite values,
  into the bounded output-stage limiter, proving it cannot panic, must fail
  non-finite input closed to silence, must keep output within the ceiling,
  and must leave the block untouched when channels or alignment are rejected.
- `drift_controller` feeds arbitrary validated occupancy PI parameters and
  occupancy snapshots into the bounded per-stream drift controller, proving
  adversarial occupancy cannot panic the controller, exceed the ratio or slew
  caps, or survive a hard resync with stale history.
- `drift_scenario` drives arbitrary bounded scenario configurations through
  the integerized occupancy model, proving an accepted configuration keeps
  every occupancy observation within the bounded PCM queue, keeps the final
  requested ratio within the configured cap, keeps each hard resync exactly
  one low- or high-side reset, and stays deterministic while arbitrary raw
  scenario fields return a clean error or satisfy the same invariants.
- `mixer_drift_scenario` drives arbitrary bounded scenario configurations
  through the multi-stream mixer-drift model at the Phase 2 four-stream upper
  bound, proving an accepted configuration keeps every stream's occupancy
  observation within the bounded PCM queue, keeps every final requested ratio
  within the configured cap, keeps each hard resync exactly one low- or
  high-side reset, keeps every stream's report equal to the single-stream
  drift-scenario result for that stream's own configuration, and stays
  deterministic while arbitrary raw scenario fields return a clean error or
  satisfy the same invariants.
- `fixed_slot_mixer` drives arbitrary insert/remove/render/push/gain scripts
  through the preallocated realtime slot table, proving membership churn cannot
  panic the mixer, bypass the fixed slot bound, or leave render output
  non-finite while the producers carry finite samples.
- `mixer_membership` publishes arbitrary insert/remove commands into the
  bounded lock-free queue shared with the callback `CommandDrivenMixer` and
  interleaves renders, finite producer pushes, and bounded gain/mute changes,
  proving the handoff boundary cannot panic, applies commands at callback
  granularity with the lowest-free-slot FIFO policy, fails closed on a full
  queue and on out-of-range removes, never exceeds the fixed slot bound, and
  keeps render output finite while producers carry finite samples.
- `mixer_jitter_scheduler` drives arbitrary bounded scenario configurations
  through the combined target-gated playout and fixed-slot mixer simulation,
  proving an accepted configuration consumes every frame through playout or
  PLC, keeps every adaptive target inside its bounded water levels, keeps
  scheduler step counters below the observation count, keeps every stream
  active in the fixed-slot mixer, and renders a finite bus while a rejected
  configuration returns a clean scenario error rather than panicking.
- `mixer_jitter_scheduler_4s` drives the same bounded scenario
  configurations at the Phase 2 four-stream upper bound: all four sources
  share one link and the full four-slot mixer table, proving the same
  consume-every-frame, target-bounds, step-counter, all-streams-active, and
  finite-bus invariants hold at the upper bound while a rejected
  configuration returns a clean scenario error rather than panicking.
- `mixer_controller_membership` drives arbitrary bounded scenario
  configurations through the controller-driven mixer membership simulation,
  proving an accepted configuration consumes every frame through playout or
  PLC, publishes bounded per-stream gain and mute events, keeps every
  per-source quality ratio within its bound, keeps the controller projection
  matched to the mixer's actual slot table through the mid-session churn,
  and renders a finite bus while a rejected configuration returns a clean
  scenario error rather than panicking.
- `multi_stream` drives arbitrary bounded scenario configurations through
  the shared-link multi-stream simulation, proving an accepted configuration
  submits exactly one packet per stream per frame on the shared link,
  accounts for every playout decision (play out or conceal exactly the
  configured frame count), and stays deterministic while a rejected
  configuration returns a clean scenario error rather than panicking.
- `multi_stream_jitter_target` drives arbitrary bounded scenario
  configurations through the shared-link adaptive-target simulation,
  proving an accepted configuration submits exactly one packet per stream
  per frame, consumes every frame through playout or PLC, observes once per
  frame, keeps every adaptive target inside its bounded `[min, max]` water
  levels, keeps step counters below the observation count, and stays
  deterministic while a rejected configuration returns a clean scenario
  error rather than panicking.
- `jitter_scheduler` drives arbitrary bounded scenario configurations
  through the target-gated playout scheduler simulation, proving an
  accepted configuration consumes every frame through playout or PLC,
  counts each scheduler tick as one gated wait or one consumed frame,
  keeps the adaptive target inside its bounded `[min, max]` water levels,
  keeps forced playouts and step counters within their bounds, completes
  inside the scenario's explicit hard deadline, and stays deterministic
  while a rejected configuration (including an expired hard deadline)
  returns a clean scenario error rather than panicking.
- `jitter_target` drives arbitrary bounded scenario configurations
  through the single-stream adaptive-target simulation, proving an
  accepted configuration submits exactly one packet per frame, consumes
  every frame through playout or PLC, observes once per frame, keeps the
  adaptive target inside its bounded `[min, max]` water levels, keeps
  step counters below the observation count, and stays deterministic
  while a rejected configuration returns a clean scenario error rather
  than panicking.
- `mixer_membership_scenario` drives arbitrary bounded scenario
  configurations through the fixed-slot mixer membership simulation,
  proving an accepted configuration consumes every frame through playout
  or PLC, submits exactly one packet per stream per frame, keeps both
  streams active after the mid-session remove/reinsert, publishes the
  two starts plus one reinsert and one remove, and renders a finite bus
  while a rejected configuration returns a clean scenario error rather
  than panicking.
- `protected_media_ingress` exercises raw input, valid authenticated arbitrary
  payloads, duplicate replay, undersized output buffers, and deterministic
  single-byte flags-byte and tag tampering plus ciphertext tampering for
  nonempty payloads. Every rejected path must clear the caller-owned plaintext
  buffer.
- `pairing_invitation` drives arbitrary begin/observe/verify_proof/expire/revoke
  scripts through the bounded pairing-invitation owner with adversarial clocks,
  lifetimes, pairing ids, sources, and per-source attempt limits, proving the
  lifecycle cannot panic, cannot desynchronize the shared session from the
  retained record, and invokes the transcript-bound proof verifier exactly when
  the record admits an attempt (never after expiry, consumption, revocation, or
  source rate limiting).
- `pairing_binding` derives public fuzz-only pairing material, exercises both
  local roles on every input, and feeds arbitrary peer-proof lengths into the
  pure TLS-binding helper. It asserts exact opposite-role verification while
  rejecting reflected and altered proofs; it does not create a TLS connection,
  QR payload, identity, pairing record, or trust state.
- `negotiation` feeds arbitrary bytes and mutations of the published Hello
  frame through control decoding into the bounded hello validation and
  capability-intersection negotiation, so adversarial version and capability
  lists cannot panic or exceed the negotiated bounds.
- `stream_decode_worker` admits already-authenticated arbitrary payloads into
  the fixed-capacity jitter buffer of a per-stream decode worker and drives a
  bounded number of decode/PLC decisions, proving adversarial post-auth bytes
  cannot panic the worker, exceed the fixed frame capacity, or grow the PCM
  queue past its bound.
- `replay_window` drives the authenticated replay window with deterministic u32 and window-width edge sequences plus fuzz-derived forward, reorder, duplicate, and far-behind operations, comparing every decision against an independent set-based model so bit-rotation or window-width bugs cannot silently change the packet-dedup admission contract.
- `reconnect_grace` drives arbitrary begin/observe/reconnected/cancel scripts
  through the bounded reconnect-grace owner with adversarial u64 clocks and
  durations, comparing every decision against an independent deadline model so
  the controller cannot panic, cannot revive a force-expired window, and always
  returns to idle after expiry, regression, reconnect, or cancel.
- `mixer_membership_controller` drives arbitrary fast-path and adapted
  admissions, removals, finite producer pushes, bounded gain and mute
  changes, renders, and drains through the control-domain admission
  boundary (ADR 0015) and asserts after every drain that the controller
  projection matches the callback fixed-slot table exactly, that pending
  commands drain to zero, that fail-closed admission (format mismatch,
  table full, command queue full) leaves both sides unchanged, and that
  render output stays finite while producers carry finite samples.
- `mixer_output_controller` drives arbitrary start, monitor, recover, stop,
  health, and fail-start scripts through the mixer output lifecycle owner
  and compares every result against an independent state model, proving the
  membership handle and output format exist exactly while the stream is
  active, a fault is reported at most once and retires both, and recovery
  reuses the last successful selection.
- `receiver_output_controller` drives arbitrary start, monitor, recover, stop,
  health, and fail-start scripts through the single-stream output recovery
  seam and compares every result against an independent state model, proving
  the platform call counts, producer replacements, published output-format
  diagnostics, and redacted snapshot stay in sync, a fault is reported at
  most once and clears the published format, and recovery reuses the last
  successful selection.
- `pcm_render_source` drives arbitrary queue pushes, epoch retirements,
  render lengths, gain, and mute scripts through the realtime PCM render
  source and its lock-free queue and compares every output slice and every
  published stream counter against an independent queue-plus-render model,
  proving partial-block preservation, the per-callback refill cap, bounded
  underflow silence, discontinuity fade-in, and gain-ramp behavior hold on
  every accepted or rejected step.
- `receiver_pipeline` drives byte-selected in-order deliveries, duplicates,
  tampered ciphertext with a valid-twin probe, retired-epoch packets, and
  rotations through the protected receiver pipeline and compares every
  outcome, plaintext zeroing, decoded PCM, and published counter against an
  independent replay oracle and counter model, proving failed AEAD never
  advances the replay window and only authenticated in-window packets reach
  the decoder.
- `udp_receiver_pipeline` drives byte-selected deliveries, rotations, fresh
  sessions, tampered ciphertext with a valid-twin probe, duplicates,
  retired-epoch packets, and oversized datagrams over real loopback sockets
  through the UDP-backed protected receiver composition and compares every
  poll outcome, pin state, decoded PCM, and diagnostic counter against an
  independent pin, replay, and counter model, proving the source pin binds
  only to the first successful protected endpoint, a changed endpoint is
  rejected before any protected work and never moves the pin, failed
  processing never binds an unbound source, and only a fresh authenticated
  session clears the binding.
- `media_session_sender` drives byte-selected sends, oversized-frame
  rejection, valid and fail-closed key-epoch rotation, fresh-session
  installation, and routing migration over real loopback sockets through the
  bounded media session sender boundary and asserts after every step that the
  sender context matches an independent model, that every accepted seal
  consumed exactly the next per-epoch sequence and reached the current peer's
  protected ingress with the expected header and plaintext, that an oversized
  frame is rejected before touching the socket without consuming a sequence,
  and that failed rotation or fresh-session replacement leaves the model
  unchanged or exactly as installed.
- `export_retention_journal` drives a deterministic edge prologue and
  arbitrary record-size scripts through the bounded export retention journal
  (ADR 0011) under input-selected count and byte bounds, comparing the
  result, retained count, retained bytes, and next sequence against an
  independent spec model that computes the retained set as the longest
  newest-first suffix, proving oversized bundles are rejected before any
  record, eviction keeps the newest exports within both fixed bounds, and
  sequence order stays FIFO on every accepted step.
- `untrusted_control_stream` feeds arbitrary bytes through the bounded
  length-prefixed control framing state machine in a deterministic per-input
  chunk size, exercising partial length and payload reads, oversize and
  zero-length rejection, and peer-close retirement.

## Run a target

Install the pinned nightly source component and run a target from this directory:

```text
rustup toolchain install nightly-2026-07-18 --component rust-src
cargo +nightly-2026-07-18 install cargo-fuzz --version 0.13.2 --locked
cargo +nightly-2026-07-18 fuzz run control_frame -- -max_len=66052
cargo +nightly-2026-07-18 fuzz run adaptive_jitter_target -- -max_len=64

cargo +nightly-2026-07-18 fuzz run media_packet -- -max_len=1264
cargo +nightly-2026-07-18 fuzz run nominal_resampler -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run channel_mapper -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run peak_limiter -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run drift_controller -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run drift_scenario -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run mixer_drift_scenario -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run fixed_slot_mixer -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run mixer_membership -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run mixer_jitter_scheduler -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run mixer_jitter_scheduler_4s -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run mixer_controller_membership -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run multi_stream -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run multi_stream_jitter_target -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run jitter_scheduler -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run jitter_target -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run mixer_membership_scenario -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run protected_media_ingress -- -max_len=1264
cargo +nightly-2026-07-18 fuzz run pairing_binding -- -max_len=4096
cargo +nightly-2026-07-18 fuzz run untrusted_control_stream -- -max_len=65540
cargo +nightly-2026-07-18 fuzz run negotiation -- -max_len=66052
cargo +nightly-2026-07-18 fuzz run control_stream_coordinator -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run control_to_mixer -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run replay_window -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run pairing_invitation -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run reconnect_grace -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run mixer_output_controller -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run mixer_membership_controller -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run receiver_output_controller -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run pcm_render_source -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run receiver_pipeline -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run udp_receiver_pipeline -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run media_session_sender -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run export_retention_journal -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run stream_decode_worker -- -max_len=64
```

`cargo-fuzz` supports LibFuzzer execution on x86-64 Unix-like systems rather
than Windows MSVC. Windows contributors should use WSL or another Linux
environment.

The configured finite Linux CI run is a build and regression smoke check, not
sustained fuzzing or native Opus-decoder evidence. Corpus, artifact, coverage,
and target outputs are intentionally ignored.
