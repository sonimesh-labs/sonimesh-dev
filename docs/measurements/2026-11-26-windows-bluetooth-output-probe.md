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
- A follow-up enumeration returned five endpoints. The default reported 48 kHz
  and eight channels; three other endpoints reported 48 kHz stereo, and one
  virtual endpoint reported 44.1 kHz stereo. The default and a non-default
  endpoint both started and stopped the selected-endpoint WASAPI loopback check.
  No listed endpoint name identified a Bluetooth output. These results verify
  endpoint metadata and loopback setup only, not Bluetooth audio capture.
- The selected default and one non-default endpoint each started and stopped a
  WASAPI loopback stream; each reported name matched the selected endpoint.
- The Windows Bluetooth discovery check passed. The inbound-wait check accepted
  the sole candidate, reported either waiting or already open, and returned to
  idle after close.
- The latest discovery HIL run reported one candidate. Its snapshot contained
  one aligned name/ID pair and passed the deterministic-order assertion; the
  test prints only the candidate count, not device names or IDs. This validates
  the current enumeration contract, not phone playback.
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
- After the fallback status copy was refined, another UI run showed the detailed
  Windows open error (`0x8007001F`) inline with the phone-wait status rather than
  as a failed-operation alert. The UI returned to `Disconnected.` after stop.
  This confirms the status presentation and cleanup; it does not verify playback.
- The probe's **Open Bluetooth settings** action opened the Windows **Bluetooth
  & devices** page from the desktop UI. Device pairing and post-pair discovery
  were not part of this check.
- Returning focus to the probe after opening Settings triggered Bluetooth
  candidate discovery and refreshed the output-endpoint list; the source picker
  appeared. This validates the settings-return refresh path, not discovery of a
  newly paired device. An already-running loopback is left alone.
- On 2026-11-26, the refreshed picker contained one playback candidate, the
  vivo X100. A direct connection attempt returned Windows error `0x8007001F`;
  no connection opened. The phone-initiated wait reached `Waiting` and was
  disconnected immediately. This verifies the current host's wait-state and
  cleanup path, not phone-initiated playback.
- The combined **Connect + monitor (auto wait fallback)** action entered the
  phone-wait state and started loopback on the selected default output. While no
  phone playback was active, loopback reported zero samples and zero peak. The
  operator disconnected; the UI returned to `Disconnected.` and offered **Start
  PCM check**, confirming the combined monitor stopped. This verifies concurrent
  startup and cleanup, not audio delivery to the endpoint.
- A separate default-output monitor run started at zero samples and zero peak.
  After the probe played its approximately two-second, low-level test tone on
  that selected endpoint, the UI reported 848,640 samples and a 4.0% recent peak
  and indicated PCM activity. Stopping the monitor returned the UI to **Start PCM
  check** and the recent peak fell to zero. This verifies that the selected
  endpoint's Windows output mix is observable and that its recent-peak display
  updates; it does not identify which application produced audio or establish a
  Bluetooth-specific PCM source.
- The Bluetooth panel's integrated **Play test tone** action was exercised while
  the selected default endpoint monitor was active. During playback, the UI
  displayed 153,600 samples, a 4.0% recent peak, and PCM activity. The monitor
  was stopped and the temporary probe closed. This verifies the integrated
  playback-to-meter path, not audible quality or Bluetooth-specific capture.
- The output meter now retains a session peak alongside its resettable recent
  peak. This allows a short signal to remain visible after a later poll clears
  recent activity; a live level bar follows the recent peak. It still reports
  the selected system mix and cannot identify the producing app.
- A rebuilt probe was exercised against the default `MCHOSE V9 Turbo+` output
  at 48 kHz, 8 channels. After the local test tone finished, a delayed read
  showed a 0.0% recent peak while retaining a 4.0% session peak across 856,320
  samples. This confirms the live meter and retained value on the selected
  endpoint; the tone came from the probe and does not establish phone playback
  or an application-specific Bluetooth PCM source.
- The local mobile-device tool exposes only an Android 9 x86_64 emulator, not a
  physical Bluetooth source, so it cannot initiate the phone-side playback test.
- The probe's UI test simulates Windows closing an opened connection. It verifies
  that the selected source returns to phone wait without stopping a jointly
  started output monitor, and that a failed restart returns to idle and stops the
  monitor. This is software state-flow coverage; a physical phone disconnect and
  reconnect cycle has not been exercised.
- Recovery from a loopback error is also covered in the UI: the selected endpoint
  is retried when present, otherwise the refreshed default is selected; no
  available endpoint leaves the error visible. These are mocked state-flow tests,
  not physical output-device removal evidence.
- A repeated hardware fallback check again returned Windows error `0x8007001F`
  for direct open, then reported the exact inbound state as `Waiting` and closed
  back to `Idle`. The ignored hardware test now prints this state for future
  runs. This confirms the host's existing direct-open limitation and fallback
  cleanup; no phone-initiated playback occurred.

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

This run does not establish phone-initiated connection, Bluetooth-specific PCM
capture, or an `ApplicationPcmSource`. The loopback test establishes that the
selected default render endpoint can be opened and closed and that a deliberately
played probe tone appears in its Windows output mix; it does not establish
source-specific capture or independently assess human audibility.
