# Deterministic network simulator

This tool is the first CI regression signal for transport and buffering logic. It
does not open sockets, use wall-clock time, emulate Wi-Fi radio behavior, or make
a physical latency claim.

## Model boundaries

- `VirtualClock` rejects backward movement and numeric overflow.
- `NetworkProfile` bounds probability fields, combined delay, and in-flight copies.
- `VirtualNetwork<T>` schedules random loss, deterministic burst loss, jitter,
  duplication, reorder delay, and capacity drops from one reproducible seed.
- `ScenarioConfig` drives sender and playout deadlines independently.
- the media-pipeline integration test crosses protocol parsing, lab-only
  authentication, replay rejection, jitter/PLC decisions, and the realtime PCM
  queue.

The lab authenticator is deliberately defined inside the test. Its marker byte is
not cryptography and cannot be linked into a production crate as a security mode.

## Run

~~~powershell
cargo run -p sonimesh-network-simulator
cargo test -p sonimesh-network-simulator
~~~

Physical WLAN, bandwidth contention, socket-buffer behavior, operating-system
scheduling, clock drift, and address migration remain separate validation work.

