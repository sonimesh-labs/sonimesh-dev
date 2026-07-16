# Development

## Prerequisites

- Rust 1.85.0 with `rustfmt` and Clippy;
- Java 21 and an Android SDK when phase 0A starts;
- Node.js 22 or newer when the desktop control UI starts;
- Windows 11 and documented audio hardware for output validation.

The current workspace is dependency-light and builds without an Android SDK,
Node package installation, audio device, or network service.

## Core checks

~~~powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo run -p sonimesh-network-simulator
~~~

On Windows, the read-only CPAL feasibility probe can enumerate output devices
without starting a stream:

~~~powershell
cargo run -p sonimesh-platform --example output_probe --features cpal-output
~~~

The following explicit lab command starts the default device for two seconds and
renders silence while recording callback sizes. It is not part of ordinary CI:

~~~powershell
cargo run -p sonimesh-platform --example output_smoke --features cpal-output
~~~

CI runs formatting, linting, and tests on Linux and Windows. These checks cover
portable protocol/audio logic only; they do not validate Android capture,
Windows callback timing, acoustic latency, Opus behavior, or physical WLANs.

## Dependency direction

~~~text
sonimesh-app            -> platform contracts, diagnostics
sonimesh-network        -> protocol primitives
sonimesh-audio          -> crossbeam-queue behind its realtime queue boundary
sonimesh-platform       -> standard library only
sonimesh-diagnostics    -> standard library only
sonimesh-protocol       -> Prost-generated bounded control schemas
desktop UI / Android    -> not bootstrapped until their validation ADR gates
~~~

Shared crates must not depend on Tauri, Vue, Compose, Android APIs, or a web
view. Platform implementations depend inward on these contracts.

## File and module size

Keep one responsibility per module. When a source file approaches roughly 300
lines, first look for an existing domain boundary rather than growing a general
utility module. Tests may remain beside compact implementation code; larger
scenario suites belong in a crate's `tests/` directory.

## Realtime review checklist

Anything reachable from `RenderSource::render` must be demonstrated to avoid:

- allocation and deallocation;
- mutexes, waits, async work, or blocking I/O;
- decoding, network access, device discovery, logs, and UI IPC;
- uninitialized output on an underflow;
- queues without a fixed capacity and observable overflow policy.
