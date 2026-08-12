# Deterministic network simulator

This tool is the first CI regression signal for transport and buffering logic. It
does not open sockets, use wall-clock time, emulate Wi-Fi radio behavior, or make
a physical latency claim.

## Model boundaries

- `VirtualClock` rejects backward movement and numeric overflow.
- `NetworkProfile` bounds probability fields, combined delay, and in-flight copies.
- `VirtualNetwork<T>` schedules random loss, non-overlapping deterministic
  burst loss, jitter, duplication, reorder delay, finite-bandwidth
  serialization, and capacity drops from one reproducible seed. Senders provide
  payload byte counts explicitly.
- `ScenarioConfig` drives sender and playout deadlines independently with a
  protocol-bounded complete media datagram size and negotiated 10/20 ms frames.
- the multi-stream scenario drives two to four source streams over one
  shared virtual link with one profile: burst loss and bandwidth
  serialization correlate across streams, while each stream keeps an
  independent jitter buffer, send cadence, and playout deadline; the
  report carries shared link counters plus per-stream jitter and PLC
  counters.
- the multi-stream jitter-target scenario runs two to four source streams
  over one shared virtual link and profile while each stream owns an
  independent ADR 0016 `AdaptiveJitterTarget`; the report carries shared
  link counters plus a per-stream target trajectory, so correlated
  shared-link loss and bandwidth serialization can be observed against
  per-stream adaptive jitter control.
- the jitter-target scenario feeds the ADR 0016 `AdaptiveJitterTarget`
  per playout decision from virtual-link arrival jitter and the
  encoded-frame water level, and reports the bounded target trajectory
  (final target, raised/lowered steps, smoothed jitter) as deterministic
  jitter-control evidence.
- the target-gated scheduler scenario makes the ADR 0016 target the
  playout decision: the scheduler waits while fewer than the target
  encoded frames are buffered, drains without gating once no frames are
  in flight, and forces a playout when the gate deadline expires, so a
  mid-stream stall stays bounded; the report carries gated ticks, forced
  playouts, and added completion latency as Phase 2 scheduler evidence.
- `DriftScenarioConfig` closes an integerized PCM-occupancy loop around the
  bounded ppm controller; it is not an ASRC or device-clock emulator.
- the media-pipeline integration test crosses protocol parsing, ChaCha20-
  Poly1305 authentication, replay rejection, jitter/PLC decisions, the bounded
  decode-worker boundary, and the realtime PCM queue. A deterministic stalled
  consumer case proves PLC is generated before the callback and overflow is
  reported at the producer boundary. The A-F `mock_worker` runner uses the same
  profile cadence with a fixed-capacity mock decoder; it is not a native Opus or
  quality experiment.
- the source-endpoint integration test applies the network-domain source pin
  around the protected receiver pipeline with virtual endpoints. It verifies
  that rejected input cannot pin, a changed endpoint cannot consume replay
  state, epoch rotation preserves the pin, failed session replacement preserves
  it, and successful fresh-session replacement permits rebinding.
- the control reconnect scenario installs non-reusable local transport tokens,
  delays a command from the retired session until after fresh hello negotiation,
  and proves that only the replacement transport can prepare new work.

## Run

~~~powershell
cargo run -p sonimesh-network-simulator
cargo test -p sonimesh-network-simulator
~~~

Physical WLAN, socket-buffer behavior, operating-system scheduling, real
ASRC/device-clock drift, TLS/socket reconnection, and operating-system/NAT
address migration remain separate validation work.
