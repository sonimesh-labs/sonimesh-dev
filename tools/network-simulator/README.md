# Deterministic network simulator

This tool is the first CI regression signal for transport and buffering logic. It
does not open sockets, use wall-clock time, emulate Wi-Fi radio behavior, or make
a physical latency claim.

## Model boundaries

- `VirtualClock` rejects backward movement and numeric overflow.
- `NetworkProfile` bounds probability fields, combined delay, and in-flight copies.
- `VirtualNetwork<T>` schedules random loss, deterministic burst loss, jitter,
  duplication, reorder delay, finite-bandwidth serialization, and capacity drops
  from one reproducible seed. Senders provide payload byte counts explicitly.
- `ScenarioConfig` drives sender and playout deadlines independently with a
  protocol-bounded complete media datagram size and negotiated 10/20 ms frames.
- `DriftScenarioConfig` closes an integerized PCM-occupancy loop around the
  bounded ppm controller; it is not an ASRC or device-clock emulator.
- the media-pipeline integration test crosses protocol parsing, ChaCha20-
  Poly1305 authentication, replay rejection, jitter/PLC decisions, the bounded
  decode-worker boundary, and the realtime PCM queue. A deterministic stalled
  consumer case proves PLC is generated before the callback and overflow is
  reported at the producer boundary. The A-F `mock_worker` runner uses the same
  profile cadence with a fixed-capacity mock decoder; it is not a native Opus or
  quality experiment.

## Run

~~~powershell
cargo run -p sonimesh-network-simulator
cargo test -p sonimesh-network-simulator
~~~

Physical WLAN, socket-buffer behavior, operating-system scheduling, real
ASRC/device-clock drift, and address migration remain separate validation work.
