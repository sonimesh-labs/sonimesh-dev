# Windows Opus native build - 2026-08-16

> Status: native libopus build and basic decode/PLC probe on the reference
> Windows host. ADR 0009 remains Provisional: Android ABI packaging, quality,
> CPU/power, 10 ms versus 20 ms comparison, and malformed-payload fuzzing
> against the native decoder are still pending.

## Summary

The candidate `opus` 0.3.1 binding (bundled libopus through `audiopus_sys`
0.2.2) built on the reference host and ran the new
`tools/opus-codec-prototype` probe. The probe encodes one deterministic
frame for each negotiated shape (10 ms and 20 ms stereo), decodes it through
the `sonimesh-audio` `StreamDecodeWorker::new_for_opus` boundary, synthesizes
one PLC block per shape, and rejects a malformed payload as a bounded
stream-local error.

## Command

~~~powershell
# From the workspace root, with the Visual Studio CMake bin on PATH:
$vsCm = 'C:\Program Files\Microsoft Visual Studio\18\Community\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin'
$env:PATH = $vsCm + ';' + $env:PATH
$env:CMAKE_POLICY_VERSION_MINIMUM = '3.5'
cargo run -p opus-codec-prototype
~~~

## Observation

| Field | Observed value |
| --- | --- |
| Local time | 2026-08-16 (Asia/Shanghai) |
| Windows version | `10.0.26200.0` |
| Rust | `rustc 1.85.0`, `cargo 1.85.0` |
| CMake | `4.2.3-msvc3` (Visual Studio 18 Community, not on PATH by default) |
| MSVC | `14.50.35717` (Visual Studio 18 Community) |
| Installed Rust targets | `x86_64-pc-windows-msvc` |
| Android SDK NDK | Absent |
| ADB connected devices | None |

## Build result

The bundled libopus CMakeLists uses a `cmake_minimum_required` floor below
CMake 3.5. CMake 4.x removed compatibility with releases older than 3.5, so
the first build attempt failed with:

```text
CMake Error at CMakeLists.txt:1 (cmake_minimum_required):
  Compatibility with CMake < 3.5 has been removed from CMake.
  Update the VERSION argument <min> value. Or, use the <min>...<max> syntax
  to tell CMake that the project requires at least <min> but has been updated
  to work with policies introduced by <max> or earlier.
  Or, add -DCMAKE_POLICY_VERSION_MINIMUM=3.5 to try configuring anyway.
```

Setting the documented environment variable `CMAKE_POLICY_VERSION_MINIMUM=3.5`
lets CMake apply the pre-3.5 policy floor, and the native build completed.
The probe then passed every step:

```text
opus_10ms: ok (encode 200 bytes; decode 960 / PLC 1920 interleaved samples; rms 0.104/0.050)
opus_20ms: ok (encode 384 bytes; decode 1920 / PLC 1920 interleaved samples; rms 0.146/0.178)
opus_malformed_payload: rejected
opus_malformed_sweep: ok (1000 payloads; 642 rejected; 358 accepted-bounded)
opus_native_build: ok
```

## Interpretation

This is evidence that the candidate binding builds natively on the reference
Windows host and that its decode and PLC paths work through the bounded
`StreamDecodeWorker` boundary with decode writing into caller-owned storage.
The decoded tone and first concealed block keep plausible finite RMS values
(0.104/0.146 decoded and 0.050/0.178 concealed), so the decode path returns
energy rather than silence or garbage. It does not qualify codec quality, CPU,
power, 10 ms versus 20 ms trade-offs, FEC, or Android ABIs, and the
malformed-payload sweep is a deterministic bounded probe (fixed-seed
xorshift32, 1,000 payloads up to the datagram limit) rather than a
coverage-guided fuzz result.

One observable detail: for an empty packet libopus synthesizes
`frame_size = output capacity / channels`, so with the reserved 20 ms block
(1,920 samples) PLC fills the whole block for both the 10 ms and 20 ms shapes.
The worker accepts that bounded length and the consumer pops it whole; a
production 10 ms stream should either size its block to 960 samples or expect
full-block PLC.

## Remaining ADR 0009 gates

- reproducible native build on the Android ABIs (`arm64-v8a`, `x86_64`) with
  a pinned NDK;
- 10 ms and 20 ms frame-duration comparison with documented quality, CPU,
  power, and loss/PLC evidence;
- authenticated malformed-payload fuzzing against the native decoder;
- physical latency and Android capture claims remain separate tests.
