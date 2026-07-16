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
- the media-pipeline integration test crosses protocol parsing, ChaCha20-
  Poly1305 authentication, replay rejection, jitter/PLC decisions, the bounded
  decode-worker boundary, and the realtime PCM queue.

## Run

~~~powershell
cargo run -p sonimesh-network-simulator
cargo test -p sonimesh-network-simulator
~~~

Physical WLAN, bandwidth contention, socket-buffer behavior, operating-system
scheduling, closed-loop clock drift, and address migration remain separate
validation work.
