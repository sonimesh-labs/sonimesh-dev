# Opus native preflight - 2026-07-20

> Status: local toolchain observation only. No Opus dependency, native build,
> ABI artifact, device load, codec-quality, power, or latency result exists.

## Command

The following read-only PowerShell probe was run from the workspace root:

```powershell
rustc -V
cargo -V
Get-Command cmake,ninja,cl,ndk-build -ErrorAction SilentlyContinue
rustup target list --installed
Test-Path "$env:ANDROID_SDK_ROOT\ndk"
adb devices -l
```

## Observation

| Field | Observed value |
| --- | --- |
| Local time | 2026-07-20 18:11:43 +08:00 |
| Windows version | `10.0.26200.0` |
| Rust | `rustc 1.85.0`, `cargo 1.85.0` |
| Installed Rust targets | `x86_64-pc-windows-msvc` only |
| `cmake` on PATH | Absent |
| `ninja` on PATH | Absent |
| `cl.exe` on PATH | Absent |
| `ndk-build` on PATH | Absent |
| Android SDK root | `C:\Users\User\AppData\Local\Android\Sdk` |
| Android SDK `ndk` directory | Absent |
| ADB connected devices | None |

## Result

The reference host cannot currently build the candidate bundled libopus path or
cross-compile/package an Android ABI. This is an observation of the configured
host and PATH at the stated time, not evidence that those tools are unavailable
elsewhere.

No ambient system library path, prebuilt binary, or dependency-only build may
be used to bypass ADR 0009's Windows/Android native-build and license evidence.
The next binding attempt requires reviewed CMake, Ninja, MSVC C/C++ tools, a
pinned Android NDK for `arm64-v8a` and `x86_64`, the selected libopus source and
license evidence, and an API 31+ Android device or emulator for load testing.
