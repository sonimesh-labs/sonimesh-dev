# Development

## Prerequisites

- Rust 1.85.0 with `rustfmt` and Clippy;
- on Linux or WSL, nightly 2026-07-18 with `rust-src` plus `cargo-fuzz`
  0.13.2 for the optional local LibFuzzer targets;
- Java 21 and an Android SDK when phase 0A starts;
- Node.js 22 or newer when the desktop control UI starts;
- Windows 11 and documented audio hardware for output validation.

The portable Rust workspace builds without an Android SDK, audio device, or
network service. The capture-only Android prototype requires the checked-in
Gradle wrapper, Java 21, Android API 36, and Android Build Tools 34.0.0.

## Core checks

~~~powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo run -p sonimesh-network-simulator
python tools/check_workspace_boundaries.py
python tools/check_android_capture_boundary.py
python tools/check_output_soak_log.py --self-test
python tools/check_output_realtime_boundary.py
python tools/check_output_lifecycle_log.py --self-test
python tools/check_output_recovery_log.py --self-test
python tools/android_capture_preflight.py --self-test
python tools/check_android_capture_record.py --self-test
Push-Location apps/android
.\gradlew.bat testDebugUnitTest --no-daemon
.\gradlew.bat :tls-provider-spike:assembleDebugAndroidTest --no-daemon
Pop-Location
~~~

## Fuzz checks

The shipping workspace does not depend on `libfuzzer-sys`; fuzz targets live in
the independent `fuzz/` workspace. From that directory:

~~~bash
rustup toolchain install nightly-2026-07-18 --component rust-src
cargo +nightly-2026-07-18 install cargo-fuzz --version 0.13.2 --locked
cargo +nightly-2026-07-18 fuzz check
cargo +nightly-2026-07-18 fuzz run control_frame -- -max_len=66052
cargo +nightly-2026-07-18 fuzz run media_packet -- -max_len=1264
cargo +nightly-2026-07-18 fuzz run protected_media_ingress -- -max_len=1264
~~~

`cargo-fuzz` supports LibFuzzer execution on x86-64 Unix-like systems rather
than Windows MSVC, so Windows contributors should run these commands through
WSL or another Linux environment. The Linux CI job is configured for 512
executions per target; that finite run is a smoke gate, not long-duration
fuzzing or native Opus-decoder evidence.

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

CI runs formatting, linting, and tests on Linux and Windows, and builds the
capture-only Android bootstrap with Java 21, API 36, and Build Tools 34.0.0.
The Android job proves a reproducible host build only; none of these checks
validate Android capture, Windows callback timing, acoustic latency, Opus
behavior, or physical WLANs.

## Dependency direction

~~~text
sonimesh-app            -> audio, network, platform contracts, protocol, diagnostics
sonimesh-network        -> protocol primitives
sonimesh-audio          -> crossbeam-queue behind its realtime queue boundary
sonimesh-platform       -> platform contracts; optional Windows CPAL feasibility adapter
sonimesh-diagnostics    -> standard library only
sonimesh-protocol       -> Prost-generated bounded control schemas
desktop UI              -> Vue/Vite control preview; Tauri bridge remains gated
Android                 -> capture-only phase 0A prototype; sender transport remains gated
~~~

Shared crates must not depend on Tauri, Vue, Compose, Android APIs, or a web
view. The desktop preview uses only low-rate state snapshots through
`apps/desktop-ui/src/bridge.ts`; it does not carry audio, keys, or realtime
callbacks. Platform implementations depend inward on these contracts.

`tools/check_workspace_boundaries.py` is the CI guard for the current direct
Rust graph. It uses Cargo metadata to enforce the six crate edges above and
rejects direct Tauri/webview, Vue, Android, JNI, and NDK dependencies in that
foundation. It intentionally cannot prove source-level execution-domain rules;
those remain code-review and test requirements.

`tools/check_android_capture_boundary.py` guards only the phase 0A production
Android source set. It rejects direct networking, TLS/crypto/Keystore, Opus,
and NDK markers in `apps/android/app/src/main`, plus matching production
dependencies and `INTERNET` in the main manifest. The ADR 0008 loopback TLS
and Keystore experiments live in the separate `:tls-provider-spike` test app.
The guard does not qualify Android capture, native codec behavior, or indirect
dependency reachability.

## Desktop UI checks

~~~powershell
Set-Location apps/desktop-ui
npm install
npm run build
npm run dev
~~~

The bundled preview bridge is intentionally not a Tauri implementation. It
exists to validate the control surface without turning pending Windows output,
pairing, or packaging work into a platform claim.

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
