# SoniMesh fuzz targets

This independent Cargo workspace contains the LibFuzzer entry points for bounded
untrusted protocol and media ingress. It is intentionally separate from the
production workspace so `libfuzzer-sys` never becomes a shipping dependency.

## Targets

- `control_frame` feeds arbitrary bytes plus mutations of the five published
  Hello, StartStream, StopStream, StreamStatus, and ControlError frames into the
  length-prefixed Protobuf control decoder.
- `media_packet` feeds arbitrary and near-valid bytes into the bounded media
  packet parser.
- `protected_media_ingress` exercises raw input, valid authenticated arbitrary
  payloads, duplicate replay, undersized output buffers, and single-byte
  header/ciphertext/tag tampering. Every rejected path must clear the
  caller-owned plaintext buffer.
- `pairing_binding` derives public fuzz-only pairing material, exercises both
  local roles on every input, and feeds arbitrary peer-proof lengths into the
  pure TLS-binding helper. It asserts exact opposite-role verification while
  rejecting reflected and altered proofs; it does not create a TLS connection,
  QR payload, identity, pairing record, or trust state.

## Run a target

Install the pinned nightly source component and run a target from this directory:

```text
rustup toolchain install nightly-2026-07-18 --component rust-src
cargo +nightly-2026-07-18 install cargo-fuzz --version 0.13.2 --locked
cargo +nightly-2026-07-18 fuzz run control_frame -- -max_len=66052
cargo +nightly-2026-07-18 fuzz run media_packet -- -max_len=1264
cargo +nightly-2026-07-18 fuzz run protected_media_ingress -- -max_len=1264
cargo +nightly-2026-07-18 fuzz run pairing_binding -- -max_len=4096
```

`cargo-fuzz` supports LibFuzzer execution on x86-64 Unix-like systems rather
than Windows MSVC. Windows contributors should use WSL or another Linux
environment.

The configured finite Linux CI run is a build and regression smoke check, not
sustained fuzzing or native Opus-decoder evidence. Corpus, artifact, coverage,
and target outputs are intentionally ignored.
