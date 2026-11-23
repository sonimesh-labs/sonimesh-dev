# Development

## Prerequisites

- Rust 1.85.0 with `rustfmt` and Clippy;
- on Linux or WSL, nightly 2026-07-18 with `rust-src` plus `cargo-fuzz`
  0.13.2 for the optional local LibFuzzer targets;
- Java 21 and an Android SDK when phase 0A starts;
- Node.js 22.22.2 when the desktop control UI starts;
- Windows 11 and documented audio hardware for output validation.
- CMake and a C toolchain for the native libopus build compiled by the
  `opus-codec-prototype` workspace member; a Visual Studio install works, and
  newer CMake releases need `CMAKE_POLICY_VERSION_MINIMUM=3.5` because the
  bundled libopus uses a pre-3.5 policy floor.

The portable Rust workspace builds without an Android SDK, audio device, or
network service; the native libopus member also needs CMake and a C toolchain
(see the Opus codec prototype section below). The capture-only Android
prototype requires the checked-in Gradle wrapper, Java 21, Android API 36, and
Android Build Tools 35.0.0. The isolated Android 17 discovery spike additionally
requires the Android API 37.0 platform.

The capture-only app keeps a bounded, in-memory summary of the latest capture
run. Once observations exist, **Copy capture evidence** places the current
status, selected usage group, observed format, duration, frame counters, and read errors on the
device clipboard for attaching to a local test report. The text is an operator
summary, not a raw-audio export or a production diagnostic bundle; it is cleared
when a new capture request begins. **Share evidence** sends the same bounded text
summary through the platform share sheet; it does not share PCM or an audio file.

Before starting, the capture-only screen lets the operator choose one playback
usage group: **Media**, **Game**, or **Other**. The choice is passed to the
foreground service for that capture session only; it does not identify the
source application, override Android capture policy, or change the observed
format reported by the service. The default is **Media**.

### Android Opus interoperability lab

The paired-audio interoperability lab exercises the real pairing, Hello and
start/stop control messages together with protected UDP and software decoding.
It uses a public fixed invitation and is intended only for a controlled lab
network. Its default is emulator and desktop loopback. From `apps/android`, build
`:opus-spike:assembleDebug` and `:opus-spike:assembleDebugAndroidTest`, then
install both APKs. Set up
`adb reverse tcp:47610 tcp:47610` and run from the repository root:

~~~powershell
cargo run -p opus-codec-prototype --bin paired-audio-lab -- --require-tone
~~~

The listener prints `SPKI_HEX` and a complete lab-only `INVITATION_HEX`. In the
Android lab activity, paste `INVITATION_HEX` into **Lab invitation payload** and
tap **Apply invitation payload** to fill the SPKI and paired control address.
Enter the UDP address separately; the payload does not replace it. The payload
contains the fixed public lab fixture secret and is not a production QR or a
persistent trust record. If using the older manual setup, copy the printed
`SPKI_HEX` into `$pin` and run:

~~~powershell
adb shell am instrument -w -e pairingSpkiHex $pin -e class 'com.sonimesh.opusspike.PairedAudioInteropTest#pairs_starts_sends_opus_and_stops' com.sonimesh.opusspike.test/androidx.test.runner.AndroidJUnitRunner
~~~

For the complete invitation path, install both APKs and pass the captured
payload directly to the instrumentation. Keep the invitation in a shell
variable rather than recording it in logs:

~~~powershell
adb shell am instrument -w -e pairingInvitationHex $invitationHex -e mediaHost 10.0.2.2 -e class 'com.sonimesh.opusspike.PairedAudioInteropTest#pairs_starts_sends_opus_and_stops' com.sonimesh.opusspike.test/androidx.test.runner.AndroidJUnitRunner
~~~

The same payload can drive the interactive capture service. Grant
`RECORD_AUDIO` on the test emulator first when the permission dialog is not part
of the run, then select the entire-screen capture option when prompted:

~~~powershell
adb shell pm grant com.sonimesh.opusspike android.permission.RECORD_AUDIO
adb shell am instrument -w -e pairingInvitationHex $invitationHex -e mediaHost 10.0.2.2 -e class 'com.sonimesh.opusspike.PairedCaptureTest#captures_tone_after_pairing_and_stops' com.sonimesh.opusspike.test/androidx.test.runner.AndroidJUnitRunner
~~~

The test synthesizes and encodes a tone; the desktop acknowledges start only
after binding its UDP receiver with the pairing-derived root and acknowledges
stop after retiring it. Successful runs report the decoded frame count and
software PCM peak. On stop, the paired lab briefly polls for trailing UDP and
plays out buffered frames before closing the receiver, with a fixed upper time
limit. UDP does not identify the final packet, so this is best effort and does
not guarantee that every frame or hardware output buffer has played. On
2026-11-22, two API 37 x86_64 emulator runs decoded 100
and 92 frames respectively, each with a peak of 0.250; the discrepancy has not
been characterized. This is not playback-capture, physical WLAN, listening,
latency, persistent trust, or production UI evidence. The interactive audio lab
also has a **Start paired UDP capture** mode: start `paired-audio-lab` without
`--require-tone`, paste its printed SPKI_HEX in the app, select that mode,
grant capture consent, and play a capturable source or use **Toggle test tone**.
**Stop capture** requests the desktop to stop the stream. This mode uses the
same fixed public lab invitation and ADB-reversed control connection; the
desktop defaults to a software PCM check. On Windows, add `--features playback`
before `--` and pass `--play` after it to use the default output device; add
`--select-output` to choose a device. `--require-tone` checks decoded audio,
not whether it was audible. Output callback counts are not a listening test.
For controlled network experiments, `--startup-buffer-ms` selects the initial
wait after the first packet, in 20 ms steps from 20 to 120 ms (default 60 ms).
The same wait is used after output recovery or decoder resynchronization. This is
a fixed startup delay, not an adaptive jitter target or an end-to-end latency
measurement; a longer wait can overflow the bounded packet buffer under bursts.
Add `--status` to print packet counts, decode counts and peak, jitter occupancy,
and output recovery state every five seconds during a session. This can help
distinguish missing media from decoded silence; counts and peak are cumulative
for the session, and they do not prove audible output.
Add `--control` alongside `--play` to adjust playback from the listener console:
enter `volume 0` through `volume 100`, `mute`, or `unmute`. These local settings
survive output recovery and successive sessions while the listener is running.
Silence can
reflect the source or platform capture policy. Other audio-lab modes below use
their separate ADB bootstrap. Remove the reverse mapping when no longer needed.

For a manually addressed controlled network, run the desktop listener with
`--bind <desktop-local-IPv4> --host <desktop-reachable-IPv4>` after `--`.
The bind address applies to TCP port 47610 and UDP port 39093; the host is the
exact address placed in the pairing invitation. Enter that same host in the
Android **Paired control IPv4** field, and the reachable desktop IPv4 in the
UDP address field. When both addresses are the same, use **Use UDP address for
control** in the lab app to copy it; the fields remain independent for ADB
reverse. Configure the host firewall as needed for this lab. No ADB
reverse is needed for this mode. The fixed public invitation is a test fixture,
not persistent trust; do not expose this lab outside a controlled network.
For an emulator, `--bind 0.0.0.0 --host 10.0.2.2` and `10.0.2.2` in both
Android fields exercise its host routing. A wildcard bind listens on all local
interfaces. This is not evidence of physical WLAN interoperability.

Add `--keep-listening` to accept successive sessions without restarting the
desktop listener or changing its displayed pin. It handles one client at a time;
each connection uses a fresh pairing exchange and media session. An unsuccessful
session returns to listening. Stop the listener with Ctrl+C. To exercise two
successive synthetic sessions, run the instrumentation method
`PairedAudioInteropTest#pairs_twice_with_continuous_listener` with the listener
in this mode and the same `pairingSpkiHex` and optional `pairingHost` arguments.

On 2026-11-22, a malformed TLS handshake was followed by two successful API 37
emulator sessions against one continuously running Windows listener using CPAL.
Both authenticated 100 packets; the first decoded 92 after eight jitter
resynchronization drops, and the second decoded 100. They recorded 593 and 261
output callbacks respectively, with no reported stream errors or oversized
callbacks. This verifies software recovery and callback activity, not listening
quality or physical-device recovery.

On 2026-11-22, API 37 x86_64 emulator tests using that direct host route and no
ADB reverse authenticated and decoded 100 synthetic frames (peak 0.250), then
captured the test tone and decoded 155 frames (peak 0.099). A separate default
loopback run injected a malformed UDP datagram during synthetic playback: the
receiver rejected one datagram and decoded all 100 authenticated frames.
These are software and emulator observations, not physical or listening results.
On 2026-11-22 an API 37 x86_64 emulator completed the paired interactive
capture flow using the app's test tone and system screen-capture consent. The
desktop acknowledged start and stop and decoded 145 frames at a software peak
of 0.095. No physical-device or hardware playback result was recorded.
With the optional Windows output enabled, the same emulator capture test on
2026-11-22 decoded 138 frames at a decoded peak of 0.093 and recorded 458 CPAL
callbacks, zero stream errors, and zero oversized callbacks. Audible output was
not independently checked; this is not a physical-device or latency result.
On the same date, the capture test also passed after adding control-connection
polling: a normal stop released the capture service and the desktop decoded 142
frames. A separate test used a local TCP proxy to close only the Android side
of the control connection while leaving the desktop UDP receiver running; the
app displayed the connection-ended status and released its capture service.
Stopping the desktop outright can instead surface a UDP send error first. A
silent network partition remains undetected until an operation reports failure;
these emulator checks do not establish physical-network behavior.

After changing the paired lab's 20 ms scheduler to advance from its previous
deadline, two synthetic-tone emulator runs each authenticated 100 packets. One
decoded 92 frames after jitter resynchronization discarded eight buffered frames;
the other decoded 100 with no resynchronization drops. Both reported peak 0.250.
These counters explain the difference in these two runs, not the earlier runs;
the cause of the resynchronization itself remains uncharacterized. A separate
playback-capture run decoded 156 of 156 authenticated packets, recorded 486 CPAL
callbacks and no stream errors or oversized callbacks. These results are not
evidence of audible quality, physical-network performance, or latency.

The synthetic sender schedules its first packet when the encoder actually
produces it, then targets a 20 ms cadence. After falling a full
frame behind, it rebases instead of emitting a catch-up burst. This keeps the
fixture's pacing representative of a live source; it does not simulate network
jitter or prove a physical sender's timing.
After this adjustment, three API 37 emulator/desktop loopback runs each
authenticated and decoded 100 packets with zero late packets and zero jitter
resynchronization drops; each reported peak 0.250. This small sample supports
the fixture change but does not establish the cause of earlier drops or qualify
long-running behavior.

On 2026-11-23, a direct API 37 emulator run using a freshly generated invitation
payload completed one synthetic session with 100 authenticated and decoded
packets, zero rejected packets, and zero decode errors. The same payload then
completed two successive sessions against one `--keep-listening` listener with
the same counters. This verifies the imported-payload and continuous-listener
lab path; it is not production pairing or physical-network evidence.

On 2026-11-23, the same imported payload drove the Android capture service
through screen-capture consent, paired startup, synthetic test-tone capture,
and normal stop. The listener authenticated and decoded 151 packets with zero
rejections, late packets, resynchronization drops, or decode errors, at a
software peak of 0.095. This is emulator loopback evidence and does not qualify
physical capture or audible output.

This isolated test encodes a synthetic 48 kHz stereo tone with Android
MediaCodec. It writes only synthetic audio to the spike application's external
files directory; it does not use capture or networking. With a single selected
emulator or device, from `apps/android` build and install the test APKs, then
run the instrumentation directly (the Gradle connected test removes its data):

~~~powershell
./gradlew.bat :opus-spike:assembleDebug :opus-spike:assembleDebugAndroidTest :opus-spike:lintDebug
adb install -r opus-spike/build/outputs/apk/debug/opus-spike-debug.apk
adb install -r opus-spike/build/outputs/apk/androidTest/debug/opus-spike-debug-androidTest.apk
adb shell am instrument -w com.sonimesh.opusspike.test/androidx.test.runner.AndroidJUnitRunner
adb pull /sdcard/Android/data/com.sonimesh.opusspike/files/tone.opusframes "$env:TEMP/sonimesh-tone.opusframes"
~~~

From the repository root, run `cargo run -p opus-codec-prototype --bin
android-interop -- "$env:TEMP/sonimesh-tone.opusframes"`. The desktop checker
requires 100 decodable 20 ms packets and a non-silent finite output. The fixture
uses big-endian 32-bit packet lengths followed by raw Opus packets; it is not a
product wire format. The API 37 x86_64 emulator produced 100 packets and 96,000
samples per channel with peak 0.250 on 2026-11-22. Other devices and capture
behavior remain unverified.

For the interactive capture-to-desktop lab, build and install `:opus-spike`'s
debug APK, connect one device through ADB, and run from the repository root:

~~~powershell
adb reverse tcp:39091 tcp:39091
cargo run -p opus-codec-prototype --features playback --bin android-live -- --play
~~~

Alternatively use `--check` without the playback feature to render into a
software check. Open **SoniMesh audio lab**, select **Start ADB capture**, grant
audio permission and fresh screen-capture consent, and play an eligible media
source. **Toggle test tone** provides a local synthetic source; **Stop capture**
or the notification action ends the stream. Restart the desktop listener for
each session. Remove the ADB mapping afterwards with `adb reverse --remove
tcp:39091`. Select the device with `adb -s SERIAL` if several are connected.

The service captures 48 kHz stereo PCM, encodes with MediaCodec, and sends
length-prefixed Opus packets through an eight-packet queue (dropping the oldest
when full). Android connects to its own loopback and ADB forwards to a listener
bound to desktop loopback. The desktop decodes off the output callback and uses
the existing bounded PCM output source. A five-second read timeout ends a stalled
session. This is an unauthenticated ADB lab transport, not the product media
protocol or a WLAN receiver. Do not expose its port through a network tunnel.
Silent output can reflect source silence or platform capture policy; it does not
identify the cause. The lab currently lacks reconnection and device switching.

On 2026-11-22 an API 37 x86_64 emulator captured the lab tone and sent 588 frames
to a Windows software render check, with decoded and rendered peaks of 0.093;
stopping capture ended the stream and removed the service. After the lab UI was
adjusted for system insets, a repeat produced 448 frames with decoded and
software-render peaks of 0.095. A separate playback
run produced non-silent decoded audio and 8,964 output callbacks with no stream
errors. No human listening confirmation or physical latency measurement was
recorded, and these runs do not establish physical-device or WLAN behavior.

For the protected UDP lab, use a separate ADB reverse mapping:

~~~powershell
adb reverse tcp:39092 tcp:39092
cargo run -p opus-codec-prototype --features playback --bin android-udp-lab -- --play
~~~

Either playback command also accepts `--select-output` after `--play`. Choose
an enumerated output before starting capture; pressing Enter selects the system
default. The chosen device and its format are fixed for that lab session. An
unavailable device or unsupported default format ends startup. The UDP lab
retries the selected output after a reported runtime fault; the ADB TCP lab
still ends on an output fault. This does not switch to a different device.

Choose **Start UDP capture** and fresh capture consent. Use `--check`
without `--features playback` for the software render check. Android generates
a new random lab secret and sends it over the ADB connection; the media packets
use ADR 0005 and the emulator's host alias to reach desktop UDP loopback port
39093. The desktop uses its protected receiver, jitter buffer, decoder worker,
and PCM output endpoint. The bootstrap is not authenticated pairing. Remove the mapping with `adb reverse --remove
tcp:39092` when finished.

For a controlled physical-device network experiment, keep ADB connected for the
bootstrap and choose the desktop's IPv4 address in the Android lab. Pass the
same local address to the desktop command with `--bind <desktop-ipv4>`; the UDP
listener uses port 39093. `--bind 0.0.0.0` explicitly listens on all IPv4
interfaces. Without `--bind`, it listens on loopback only. The Android default
`10.0.2.2` is the emulator host alias, not an address for a physical phone.
Windows firewall rules and Android local-network access may need attention on a
physical device. This ADB-assisted lab is not product discovery or pairing.
The Android lab remembers the last valid UDP destination on this device. Add
`--repeat` to the UDP command to wait for another session after a capture stops
or fails; each session uses a fresh secret and requires new capture consent.
Press Ctrl+C to stop the listener. In playback mode the output chosen at startup
is reused for subsequent sessions; restart the command to choose another output.
Add `--control` to accept `volume 0` through `volume 100`, `mute`, and `unmute`
on standard input while the UDP lab runs. Volume is a linear attenuation
percentage. Changes also apply to subsequent `--repeat` sessions; EOF stops
accepting commands without stopping audio. In `--check` mode the setting affects
the software render check. Select an output before entering control commands.

The lab screen shows connection and capture status, including a desktop
connection failure or disconnect and a stopped projection. Status is kept in
memory while the lab process remains alive; it is not a persistent session log.
After two seconds of silent captured frames it also reports that no capturable
audio was detected, with possible causes; a later non-silent read restores the
capturing status. This reflects capture input, not delivery or audible playback.

On 2026-11-22 the API 37 x86_64 emulator passed the published Android-to-desktop
media crypto vector. In a capture run the desktop processed 502 authenticated
packets, decoded 499 frames, concealed one frame, and rendered a peak of 0.094.
In a separate playback run it processed 519 packets, decoded 491 frames,
concealed 28 frames, and recorded 1,112 output callbacks with no stream errors.
The processed counts in those runs include any packets later rejected by jitter
admission; the updated command reports late packets separately. These short runs
do not establish audio quality, stable scheduling, physical WLAN, or listening
results. The laboratory's fixed 20 ms scheduler and clock drift still need work.

The original scheduler advanced through an empty jitter buffer and could then
classify subsequent packets as late. After changing it to wait for buffered
data, a further software run admitted 602 packets, decoded 600 frames, reported
zero late packets and zero concealed frames, and had 13 empty ticks. The render
peak was 0.093. A real sequence gap still invokes concealment in the regression
test. This single short run does not establish long-term clock stability.

On 2026-11-22 an emulator run selected a non-default Windows output: the UDP
lab admitted 492 packets, decoded 484 frames, observed one resynchronization
and 38 empty ticks, and recorded 1,150 callbacks with no stream errors. A
separate ADB TCP run on that endpoint decoded 394 frames with a 0.117 peak and
recorded 919 callbacks with no stream errors. Neither run included a human
listening assessment or a physical latency measurement.

A subsequent API 37 emulator run entered the desktop's network-interface IPv4
address and bound the UDP receiver to that address. It admitted 417 packets,
decoded 412 frames, had three empty ticks and no late packets or resynchronizations,
and reached a software-render peak of 0.096. This used an emulator and does not
establish physical-device WLAN behavior.

On 2026-11-22 an API 37 x86_64 emulator ran two successive UDP capture
sessions against one `--check --repeat` desktop process, granting new screen
capture consent for each. The sessions admitted 338 and 371 packets, decoded
335 and 368 frames, and reached software-render peaks of 0.097 and 0.100;
both reported zero late packets, resynchronizations, concealed frames, and empty
ticks. The listener returned to waiting after each stop. This short ADB-assisted
emulator run is not physical WLAN, listening, or latency evidence.

When a local SDK is missing one of those components, install the same set the
Android CI job installs before running the Gradle commands:
`sdkmanager "platform-tools" "platforms;android-36" "platforms;android-37.0" "build-tools;35.0.0"`
(after accepting the licenses). Android Gradle Plugin 8.13.2 defaults to Build
Tools 35.0.0, so a missing local build-tools component fails the build even
when the platforms packages are present.

### Windows desktop output probe

The isolated Tauri prototype displays the Windows output endpoints and default
formats returned by the CPAL adapter, and can play a short test tone after the
operator selects an endpoint. It does not control a receiver or change the
system default output. If one endpoint's format cannot be read, it remains listed with an
unavailable-format indication. Build the web assets and run the prototype from
the repository root:

~~~powershell
Push-Location apps/desktop-ui
npm ci
npm run build
Pop-Location
cargo +1.98.1 run --manifest-path tools/tauri-output-prototype/Cargo.toml --locked
~~~

This separate prototype uses Rust 1.98.1 for its locked Tauri dependencies; the
shared workspace remains on Rust 1.85.0. Windows WebView2 is required. The window
is not the receiver control UI and does not verify receiver playback, latency, or pairing.
Its application icon uses the Feather `headphones` icon (MIT license).
On 2026-11-22, a Windows desktop debug build opened this window and displayed
five CPAL output endpoints, their formats, and the default marker through the
Tauri IPC path; a refresh retained the five listed endpoints. This is a
read-only enumeration check, not playback evidence.
On the same date, a test tone was requested through the window on a virtual
Windows output endpoint; the CPAL stream reported callbacks without errors and
the UI reported completion. Audibility was not assessed.

## Core checks

~~~powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo run -p sonimesh-network-simulator
python tools/check_workspace_boundaries.py
python tools/check_workspace_members.py
python tools/check_doc_links.py
python tools/check_doc_dates.py
python tools/check_fuzz_targets.py
python tools/check_adr_index.py
python tools/check_preview_alignment.py
python tools/check_pairing_vectors.py
python tools/check_pairing_qr_vectors.py
python tools/check_pairing_proof_frame.py
python tools/check_control_frame_vectors.py
python tools/check_media_header_vectors.py
python tools/check_media_key_vectors.py
python tools/check_negotiation_vectors.py
python tools/check_media_crypto_vectors.py
python tools/check_sequence_replay.py
python tools/check_media_limits.py
python tools/check_android_capture_boundary.py
python tools/check_output_soak_log.py --self-test
python tools/check_output_realtime_boundary.py
python tools/check_output_lifecycle_log.py --self-test
python tools/check_output_recovery_log.py --self-test
python tools/android_capture_preflight.py --self-test
python tools/android_discovery_preflight.py --self-test
python tools/check_android_capture_record.py --self-test
python tools/check_android_discovery_record.py --self-test
python tools/check_ci_wiring.py
Push-Location apps/android
.\gradlew.bat lintDebug testDebugUnitTest assembleDebug --no-daemon
.\gradlew.bat :pairing-protocol:lint :pairing-protocol:assemble --no-daemon
.\gradlew.bat :pairing-client:lint :pairing-client:testDebugUnitTest --no-daemon
.\gradlew.bat :discovery-spike:lintDebug :discovery-spike:assembleDebugAndroidTest --no-daemon
.\gradlew.bat :tls-provider-spike:assembleDebugAndroidTest --no-daemon
Pop-Location
Push-Location apps/desktop-ui
npm test
npm run build
Pop-Location
~~~

With an Android device or emulator connected, run the pairing codec's published
vector and copy-ownership tests with
`./gradlew.bat :pairing-protocol:connectedDebugAndroidTest` from `apps/android`.
The library provides codecs only; the capture-only app does not use them at
runtime and this check does not exercise scanning or a pairing connection.

To exercise the Android pairing client with the desktop invitation listener on
an emulator, build and install the TLS spike APKs from `apps/android`:

~~~powershell
.\gradlew.bat :tls-provider-spike:assembleDebug :tls-provider-spike:assembleDebugAndroidTest --no-daemon
adb install -r tls-provider-spike/build/outputs/apk/debug/tls-provider-spike-debug.apk
adb install -r tls-provider-spike/build/outputs/apk/androidTest/debug/tls-provider-spike-debug-androidTest.apk
adb reverse tcp:47610 tcp:47610
~~~

Start `cargo run -p windows-tls-provider-prototype --bin pairing-lab` from the
repository root. In another terminal, set `$pin` to the printed public
`SPKI_HEX` and run one instrumentation method (restart the one-shot desktop
listener for each method):

~~~powershell
adb shell am instrument -w -r -e class 'com.sonimesh.tlsproviderspike.security.PairingClientInteropTest#pairs_with_desktop_invitation_listener' -e pairingSpkiHex $pin com.sonimesh.tlsproviderspike.test/androidx.test.runner.AndroidJUnitRunner
~~~

The `rejects_a_changed_desktop_pin` method expects the desktop to exit with a
TLS error. The positive test also exchanges Hello messages and confirms the
20 ms capability on both sides. This lab uses a fixed public secret and ADB
loopback; it does not implement camera scanning, persistent trust, stream
lifecycle or physical WLAN validation. The QR expiry is interpreted by the
desktop invitation owner, whose monotonic clock is not comparable to the Android
device clock.

The vector pin checkers (`check_pairing_vectors.py`, `check_control_frame_vectors.py`,
`check_media_header_vectors.py`, `check_media_key_vectors.py`,
`check_negotiation_vectors.py`, `check_media_crypto_vectors.py`,
`check_sequence_replay.py`, and `check_media_limits.py`) reimplement
each documented version-one protocol boundary in pure standard-library Python and
cross-check the pins published in the protocol docs, the Rust tests, and the
Kotlin tests. The sequence/replay verifier reimplements the sender-side
`MediaSequence` exhaustion guard and the receiver-side 64-packet `ReplayWindow`
and cross-checks the Rust and Kotlin mirrors against the published boundary
semantics. `check_media_limits.py` re-reads the fixed media-plane and
control-frame numeric bounds (frame prefix, header, extended-header,
datagram, tag, plaintext, frame, start-stream, detail, and media-key epoch limits)
from the Rust sources, the Kotlin mirrors, and the `docs/mvp-baseline.md`
capacities table, and recomputes the derived bounds.
`check_ci_wiring.py` verifies
that every `tools/` checker is wired into CI with its required
mode, that the contributor-facing Core checks block documents the same tools with the
same modes, and that every Cargo command and Gradle task listed in that block also runs
in CI. `check_doc_links.py` verifies that every relative Markdown link in the
repository-owned documentation resolves to an existing file, skipping generated directories
and fenced code blocks. `check_doc_dates.py` verifies that every `> Updated:` date
declared by a design or measurement document is not older than the newest git commit that
changed the document's content, treating pure date-line refreshes as non-content so a date
edit does not immediately look stale. `check_workspace_members.py` keeps the Cargo
`[workspace] members` list in sync with the package manifests on disk so a new crate cannot
silently fall out of the workspace-wide test and lint gates.

## Fuzz checks

The shipping workspace does not depend on `libfuzzer-sys`; fuzz targets live in
the independent `fuzz/` workspace. From that directory:

~~~bash
rustup toolchain install nightly-2026-07-18 --component rust-src
cargo +nightly-2026-07-18 install cargo-fuzz --version 0.13.2 --locked
cargo +nightly-2026-07-18 fuzz check
cargo +nightly-2026-07-18 fuzz run control_frame -- -max_len=66052
cargo +nightly-2026-07-18 fuzz run adaptive_jitter_target -- -max_len=64
cargo +nightly-2026-07-18 fuzz run media_packet -- -max_len=1264
cargo +nightly-2026-07-18 fuzz run nominal_resampler -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run polyphase_resampler -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run channel_mapper -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run peak_limiter -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run drift_controller -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run drift_scenario -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run mixer_drift_scenario -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run fixed_slot_mixer -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run mixer_membership -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run mixer_jitter_scheduler -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run mixer_jitter_scheduler_4s -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run mixer_controller_membership -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run multi_stream -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run multi_stream_jitter_target -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run jitter_scheduler -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run jitter_target -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run target_gated_playout -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run mixer_membership_scenario -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run protected_media_ingress -- -max_len=1264
cargo +nightly-2026-07-18 fuzz run pairing_binding -- -max_len=4096
cargo +nightly-2026-07-18 fuzz run pairing_qr -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run pairing_proof -- -max_len=128
cargo +nightly-2026-07-18 fuzz run untrusted_control_stream -- -max_len=65540
cargo +nightly-2026-07-18 fuzz run negotiation -- -max_len=66052
cargo +nightly-2026-07-18 fuzz run stream_decode_worker -- -max_len=64
cargo +nightly-2026-07-18 fuzz run stream_format_adapter -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run native_opus_decode -- -max_len=1200
cargo +nightly-2026-07-18 fuzz run control_stream_coordinator -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run control_to_mixer -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run replay_window -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run pairing_invitation -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run reconnect_grace -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run mixer_output_controller -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run mixer_membership_controller -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run receiver_output_controller -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run pcm_render_source -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run receiver_pipeline -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run udp_receiver_pipeline -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run media_session_sender -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run export_retention_journal -- -max_len=1024
cargo +nightly-2026-07-18 fuzz run media_session_allocator -- -max_len=64
~~~

Native Windows MSVC execution is supported and verified: install any current
nightly toolchain with `cargo-fuzz`, and put the MSVC ASan runtime DLL
(`clang_rt.asan_dynamic-x86_64.dll`, shipped under
`<VS>\VC\Tools\MSVC\<ver>\bin\Hostx64\x64`) on `PATH` before running a
sanitizer build, because the linked binary loads it at startup. Targets that
compile the bundled libopus through CMake additionally need the
`CMAKE_POLICY_VERSION_MINIMUM=3.5` environment variable described above. The
Linux CI job is configured for 512 executions per target; that finite run is a
smoke gate, not long-duration fuzzing or native Opus-decoder evidence.

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

Set SONIMESH_OUTPUT_SMOKE_DEVICE_ID to an opaque endpoint identifier to target a
pinned output device instead of the current default; an empty or non-Unicode
value is rejected, and an unknown identifier fails closed. Both the CPAL smoke
and the direct-WASAPI prototype smoke accept the same identifier, with or
without the `wasapi:` prefix. This keeps the same bounded callback measurement
available for non-default endpoints such as a specific Realtek or USB device.

The long-running CPAL soak (`output_soak`) and the direct-WASAPI soak
(`wasapi_output_soak`) accept the same opaque endpoint identifier through
SONIMESH_OUTPUT_SOAK_DEVICE_ID, with the same empty/non-Unicode rejection and
fail-closed unknown-device behavior, so an eight-hour soak can target a pinned
endpoint instead of the current default on either backend.

The repeated lifecycle smokes (`output_lifecycle_smoke` and
`wasapi_lifecycle_smoke`) accept the same opaque endpoint identifier through
SONIMESH_OUTPUT_LIFECYCLE_DEVICE_ID, with the same empty/non-Unicode rejection
and fail-closed unknown-device behavior, so start/stop cycle evidence can also
target a pinned endpoint on either backend.

The `tools/latency-measurement` crate provides the deterministic MLS reference
signal and correlation offset estimator used by the acoustic timing workflow.
It has no dependencies and is covered by the ordinary `cargo test --workspace`
gate; run `cargo run -p sonimesh-latency-measurement -- --help` for the
`generate`, `measure`, `info`, and `summary` commands. It is a software
measurement aid, not a physical latency proof.

The `tools/opus-codec-prototype` workspace member compiles the candidate
`opus` binding and exercises it through the `sonimesh-audio`
`StreamDecodeWorker::new_for_opus` boundary: it encodes one deterministic
stereo frame for each negotiated 10 ms and 20 ms shape, decodes each, and
synthesizes one PLC block per shape, then rejects a malformed payload as a
bounded stream-local error, checks decoded and concealed RMS sanity, and sweeps
1,000 deterministic malformed payloads that must stay bounded. Run it with:

~~~powershell
cargo run -p opus-codec-prototype
~~~

Its Windows native build needs CMake on PATH and, on CMake releases that
removed pre-3.5 compatibility, the `CMAKE_POLICY_VERSION_MINIMUM=3.5`
environment variable; the Windows CI step sets that variable. This run is
native-build and basic decode/PLC evidence for ADR 0009 only; it is not a
quality, power, latency, Android ABI, or malformed-payload fuzz result.

For a local end-to-end media-path exercise, the `local-audio` binary in the
same prototype sends a generated tone through native Opus, encrypted UDP
loopback, the receiver and its render source. `--check` runs without audio
hardware. On Windows, `--play` opens the default output device:

~~~powershell
cargo run -p opus-codec-prototype --bin local-audio -- --check
cargo run -p opus-codec-prototype --features playback --bin local-audio -- --play
~~~

This is a single-process lab with a public fixed test key and loopback sockets;
it is not phone capture, pairing, physical WLAN or a production session.

CI runs formatting, linting, and tests on Linux and Windows, and builds the
capture-only Android bootstrap with Java 21, API 36, and Build Tools 35.0.0.
The Android job proves a reproducible host build only; none of these checks
validate Android capture, Windows callback timing, acoustic latency, Opus
behavior, or physical WLANs.

## Dependency direction

~~~text
sonimesh-app            -> audio, network, platform contracts, protocol, diagnostics
sonimesh-network        -> protocol primitives
sonimesh-audio          -> diagnostics per-slot counters; crossbeam-queue behind its realtime queue boundary
sonimesh-platform       -> platform contracts; optional Windows CPAL feasibility adapter
sonimesh-diagnostics    -> standard library only
sonimesh-protocol       -> Prost-generated bounded control schemas
desktop UI              -> Vue/Vite control preview plus injectable low-rate Tauri adapter
Android                 -> capture-only phase 0A prototype; sender transport remains gated
~~~

Shared crates must not depend on Tauri, Vue, Compose, Android APIs, or a web
view. The desktop preview uses only low-rate state snapshots through
`apps/desktop-ui/src/bridge.ts`; it does not carry audio, keys, or realtime
callbacks. Platform implementations depend inward on these contracts.

`tools/check_workspace_boundaries.py` is the CI guard for the current direct
Rust graph. It uses Cargo metadata to enforce the six crate edges above and
rejects direct Tauri/webview, Vue, Android, JNI, and NDK dependencies in that
foundation. It also enforces the documented standard-library-only contracts by
rejecting any direct, dev, or build dependency in `sonimesh-diagnostics` and
`tools/latency-measurement`. It intentionally cannot prove source-level
execution-domain rules; those remain code-review and test requirements.

`tools/check_android_capture_boundary.py` guards only the phase 0A production
Android source set. It rejects direct networking, TLS/crypto/Keystore, Opus,
and NDK markers in `apps/android/app/src/main`, plus matching production
dependencies and `INTERNET` in the main manifest. The ADR 0008 loopback TLS
and Keystore experiments live in the separate `:tls-provider-spike` test app.
The guard does not qualify Android capture, native codec behavior, or indirect
dependency reachability.

## Cross-platform TLS lab probe

The `interop-server` binary and `DesktopInteropTlsTest` are an opt-in lab
experiment for ADR 0021. The server listens on `127.0.0.1:47610` by default,
generates a fresh certificate, prints its public SPKI and waits at most 30
seconds for one connection. Start it only when the test is ready:

~~~powershell
cargo run -p windows-tls-provider-prototype --bin interop-server
~~~

For an attached emulator, use `adb reverse tcp:47610 tcp:47610`, then run the
instrumentation test by its fully qualified class name, supplying `desktopHost`
as `127.0.0.1`, `desktopPort` as `47610`, and `spkiHex` as the server's printed
SPKI. A direct emulator invocation is:

~~~powershell
adb reverse tcp:47610 tcp:47610
adb shell am instrument -w `
  -e desktopHost 127.0.0.1 `
  -e desktopPort 47610 `
  -e spkiHex <SPKI_HEX_FROM_SERVER> `
  com.sonimesh.tlsproviderspike.test/androidx.test.runner.AndroidJUnitRunner
~~~

For a physical device the server requires an explicit reachable bind address
and the client needs the corresponding host and port. Both the Android test and
server must succeed. The fixed public lab secret provides no real peer
authentication; never reuse this probe's inputs in a product session. Ordinary
instrumentation runs skip the interop test when no arguments are supplied.
An emulator/ADB run is not WLAN or physical-device evidence and does not close
ADR 0021's cross-platform gate.

## Desktop UI checks

~~~powershell
Set-Location apps/desktop-ui
npm install
npm test
npm run build
npm run dev
~~~

Use `npm ci` instead of `npm install` when reproducing the CI dependency graph.
CI runs `npm ci`, `npm test`, and `npm run build` on current Ubuntu and
Windows runners with Node.js 22.22.2. `npm test` and `npm run build` are the
enforced gates repeated in the Core checks pre-CI contract; `npm install` and
`npm run dev` are local-dev conveniences only. This validates the deterministic
preview UI tests and production bundle only; it does not validate a live Tauri
shell, receiver owner, audio, pairing, or platform capability.

The bundled preview bridge remains the default browser implementation. The
injectable `TauriDesktopBridge` only maps low-rate commands to a shell-provided
`invoke` function; it does not create a receiver owner or carry audio, keys, or
realtime callbacks. The shell-neutral `DesktopControlOwner` now provides the
low-rate command owner for lifecycle, output, stream, and diagnostics actions;
a live Tauri shell and its command bindings remain pending platform integration.
Control transports can use `execute_control_action` to route negotiated and
tokenized stream actions without duplicating start/rotate/stop dispatch logic.
`DesktopReceiverOwner` is the next composition boundary: it stores that owner,
the injected receiver pipeline, and optional stream counters together, while
leaving decoder, socket, and output construction to the application layer. Its
UDP specialization exposes one nonblocking packet poll and one decode/PLC step
per scheduler tick so the future shell can drive media work outside the output
callback. `DesktopReceiverOwner::tick_media` now combines those two operations
and returns both the packet admission and decode/PLC decision, so a scheduler
does not need to reconstruct the `arrivals_pending` input itself.
After pairing, `DesktopControlSession` is the shell-neutral transport adapter:
construct it with `DesktopControlSession::from_trusted_channel` and the
installed owner session token, call `poll_once` from the control loop, and call
`flush` when a nonblocking write reports `Pending`. It executes one bounded
envelope per tick, returns a correlated Hello with the selected version and
common capabilities, returns lifecycle statuses for start/rotate/stop, and returns a
correlated wire error for an owner rejection. Completed duplicate start/stop
requests receive status replies; pending duplicates and starts with unavailable
or not-yet-readmitted output receive retryable errors. After recreating output,
`DesktopReceiverOwner::recover_stream` reconnects a persisted stream to its
owned pipeline. It is still a composition
boundary, not a live Tauri shell or a proof of Windows audio output.
`DesktopReceiverScheduler` now owns that receiver owner plus an optional control
channel, automatically retires a closed/error channel, and exposes the UDP
`tick_media` operation through the same scheduling boundary.
Before each control poll it flushes any reply left by a previous nonblocking
write; a full socket therefore pauses command intake for one tick instead of
turning a recoverable `Pending` write into a dropped session. The scheduler
retains its framing buffers across polls, so fragmented incoming frames and
partially written replies resume where the previous poll stopped. Explicitly
detaching a channel discards any unfinished frame or reply.
The UDP specialization also exposes `tick`, which runs one control poll and one
media poll/decode together, then pumps decoded PCM into the mixer until each
output queue is full or its source underflows. Inspect `pump()` alongside the
independent control and media results; a control or UDP error does not suppress
the pump. Each call performs a decode/PLC decision, so its caller must provide
the appropriate playout cadence. This is not a clock-driven runtime or a burst
drainer; callers needing faster packet polling must schedule that separately.
When a control poll returns `Closed` or an unrecoverable transport error, the
caller should invoke `mark_control_lost(now_ms, grace_duration_ms)` once. That
retires pending control work, moves the lifecycle to `Reconnecting`, and arms
the bounded reconnect window while leaving the UDP receiver available for the
same scheduler loop.
The UDP specialization also exposes `tick_at(now_ms)`, which observes reconnect
grace before running that same control/media work and returns the lifecycle,
control, media, and output-health results independently. A grace expiry therefore
retires the output before the tick continues, while a clock error or output
observation failure still does not suppress the media poll.
After a fresh trusted channel is accepted, call `attach_reconnected_control`
(`attach_trusted_reconnected_control` for the TLS wrapper). It clears the grace
window, returns the lifecycle to `Ready`, issues a fresh control token, and gives
the scheduler ownership of the new channel.
The composable polls the same low-rate snapshot command once per second and clears
that timer on unmount, so reconnect countdowns and asynchronous output faults do
not depend on a user clicking Refresh.

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
