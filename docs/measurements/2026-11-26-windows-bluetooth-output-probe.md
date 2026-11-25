# Windows Bluetooth and selected output probe result

> Status: local Windows device evidence; Bluetooth application PCM remains unverified
> Updated: 2026-11-26

## Environment

| Field | Observed value |
| --- | --- |
| Windows | Windows 11 Home, build 26200, x64 |
| Rust | 1.98.1 (`x86_64-pc-windows-msvc`) |
| CPAL | 0.18.2, Windows WASAPI host |
| Test source | One `AudioPlaybackConnection` candidate; device name omitted |

## Results

- CPAL output endpoint enumeration returned at least one endpoint, and every
  listed ID could be resolved again through `HostTrait::device_by_id`.
- The selected default and one non-default endpoint each started and stopped a
  WASAPI loopback stream; each reported name matched the selected endpoint.
- The Windows Bluetooth discovery check passed. The inbound-wait check accepted
  the sole candidate, reported either waiting or already open, and returned to
  idle after close.
- A combined check started the default WASAPI loopback monitor and Bluetooth
  inbound wait together, observed waiting or open, then stopped both. It passed
  without playing audio; this confirms the two operations can coexist, not that
  Bluetooth audio reaches the monitored output.
- A direct open attempt returned `UnknownFailure` with Windows extended error
  `0x8007001F`. The connection was closed by the implementation after failure.
  The remote source's state was not established, so the cause remains unknown.
  Retrying through the API's async projection returned the same status and error;
  the implementation remains on the synchronous projection.
- A recovery check reproduced that direct-open failure, refreshed the same sole
  candidate, then entered inbound wait and closed cleanly. This verifies the
  fallback connection flow after failure; it does not verify phone-initiated
  playback.
- The current desktop probe build refreshed the sole candidate in its UI. The
  operator selected it and started **Connect or wait for phone**. The UI showed
  `Waiting for the phone to connect`, then returned to `Disconnected.` after the
  operator disconnected. This exercises the direct-open failure fallback through
  the desktop command path and verifies cleanup. No phone playback was started,
  so it does not verify audible playback or a Bluetooth PCM source.
- The combined **Connect + monitor (auto wait fallback)** action entered the
  phone-wait state and started loopback on the selected default output. While no
  phone playback was active, loopback reported zero samples and zero peak. The
  operator disconnected; the UI returned to `Disconnected.` and offered **Start
  PCM check**, confirming the combined monitor stopped. This verifies concurrent
  startup and cleanup, not audio delivery to the endpoint.
- The local mobile-device tool exposes only an Android 9 x86_64 emulator, not a
  physical Bluetooth source, so it cannot initiate the phone-side playback test.

Commands run from the repository root:

~~~powershell
cargo +1.98.1 test --offline -j 1 --manifest-path tools/tauri-output-prototype/Cargo.toml listed_output_endpoints_can_be_reopened_by_id -- --ignored --nocapture
cargo +1.98.1 test --offline -j 1 --manifest-path tools/tauri-output-prototype/Cargo.toml selected_default_endpoint_loopback_starts_and_stops -- --ignored --nocapture
cargo +1.98.1 test --offline -j 1 --manifest-path tools/tauri-output-prototype/Cargo.toml selected_nondefault_endpoint_loopback_starts_and_stops -- --ignored --nocapture
cargo +1.98.1 test --offline -j 1 --manifest-path tools/tauri-output-prototype/Cargo.toml windows_discovery_returns_a_usable_snapshot -- --ignored --nocapture
cargo +1.98.1 test --offline -j 1 --manifest-path tools/tauri-output-prototype/Cargo.toml real_windows_inbound_wait_reports_state_and_closes -- --ignored --nocapture
cargo +1.98.1 test --offline -j 1 --manifest-path tools/tauri-output-prototype/Cargo.toml bluetooth_wait_and_default_endpoint_monitor_run_together -- --ignored --nocapture
cargo +1.98.1 test --offline -j 1 --manifest-path tools/tauri-output-prototype/Cargo.toml direct_open_failure_can_fall_back_to_inbound_wait -- --ignored --nocapture
cargo +1.98.1 test --offline -j 1 --manifest-path tools/tauri-output-prototype/Cargo.toml real_connection_opens_reports_state_and_closes -- --ignored --nocapture
~~~

This run does not establish audible playback, phone-initiated connection,
Bluetooth-specific PCM capture, or an `ApplicationPcmSource`. The loopback test
only establishes that the selected default render endpoint can be opened and
closed; no audio was deliberately played during the test.
