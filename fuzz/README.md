# SoniMesh fuzz targets

This independent Cargo workspace contains the LibFuzzer entry points for bounded
untrusted protocol and media ingress. It is intentionally separate from the
production workspace so `libfuzzer-sys` never becomes a shipping dependency.

## Targets

- `adaptive_jitter_target` feeds arbitrary arrival ticks and sender sample
  timestamps into the relative arrival-jitter estimator and then the adaptive
  target controller with arbitrary buffered-frame counts, proving adversarial
  timing cannot panic the controller, move the target outside its bounded
  `[min, max]` water levels, or corrupt the reset path.
- `control_stream_coordinator` drives the real pairing path to a trusted ready session, installs one authenticated transport, and feeds arbitrary decoded control frames plus mutations of the published Hello, StartStream, and StopStream frames through the coordinator, committing or aborting every produced Start, Rotate, and Stop action with its token. It proves authenticated adversarial bytes cannot panic the coordinator, admit media past `MAX_ACTIVE_STREAMS`, or complete work across a session-expiry or reconnect boundary.
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
- `fixed_slot_mixer` drives arbitrary insert/remove/render/push/gain scripts
  through the preallocated realtime slot table, proving membership churn cannot
  panic the mixer, bypass the fixed slot bound, or leave render output
  non-finite while the producers carry finite samples.
- `protected_media_ingress` exercises raw input, valid authenticated arbitrary
  payloads, duplicate replay, undersized output buffers, and deterministic
  single-byte flags-byte and tag tampering plus ciphertext tampering for
  nonempty payloads. Every rejected path must clear the caller-owned plaintext
  buffer.
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
cargo +nightly-2026-07-18 fuzz run fixed_slot_mixer -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run protected_media_ingress -- -max_len=1264
cargo +nightly-2026-07-18 fuzz run pairing_binding -- -max_len=4096
cargo +nightly-2026-07-18 fuzz run untrusted_control_stream -- -max_len=65540
cargo +nightly-2026-07-18 fuzz run negotiation -- -max_len=66052
cargo +nightly-2026-07-18 fuzz run control_stream_coordinator -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run replay_window -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run stream_decode_worker -- -max_len=64
```

`cargo-fuzz` supports LibFuzzer execution on x86-64 Unix-like systems rather
than Windows MSVC. Windows contributors should use WSL or another Linux
environment.

The configured finite Linux CI run is a build and regression smoke check, not
sustained fuzzing or native Opus-decoder evidence. Corpus, artifact, coverage,
and target outputs are intentionally ignored.
