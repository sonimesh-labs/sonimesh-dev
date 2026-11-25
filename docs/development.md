# Development

## Prerequisites

- Rust 1.85.0 with `rustfmt` and Clippy;
- on Linux or WSL, nightly 2026-07-18 with `rust-src` plus `cargo-fuzz`
  0.13.2 for the optional local LibFuzzer targets;
- Java 21 and an Android SDK when phase 0A starts;
- Node.js 22.22.2 when the desktop control UI starts;
- Windows 11 and documented audio hardware for output validation.
- Rust 1.88 for the standalone Tauri desktop shell under
  `apps/desktop-ui/src-tauri`; the main Rust workspace remains compatible with
  Rust 1.85.
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
On 2026-11-23, an API 37 x86_64 emulator capture was stopped after 27.7 seconds;
the screen retained the observed 48 kHz stereo format, buffer sizes, and final
frame counters. This silent emulator run checks the display and lifecycle path,
not physical-device capture or audio playback.
While a capture is active, the foreground notification also shows elapsed
capture duration in its subtext and updates from the same bounded evidence
clock. This keeps the run's liveness visible when the activity is off-screen;
the notification still exposes only control text and no audio data. Tapping its
content action returns to the capture activity without changing the running
service.

The capture-only screen places its controls immediately after the status card;
session metrics and evidence follow below them. During capture, the stop and
test-tone controls appear before the disabled source selector.
Before starting, the capture-only screen lets the operator choose **Media**,
**Game**, **Other**, or **All supported** (the three groups together).
Media remains the default. The choice is passed to the
foreground service for that capture session only; it does not identify the
source application, override Android capture policy, or change the observed
format reported by the service.
While capture is active, **Start test tone** plays a low-amplitude local tone using the
current session's selected usage; **All supported** uses Media. Stop it with the
same control. The tone also stops when capture ends or the activity leaves the
foreground. Use the signal and evidence counters to check this app's own audio;
success does not mean another application permits playback capture. Device and
system audio policy still apply.
**Check supported usages** runs one short local tone for Media, Game, and Other
in the same capture session and reports whether each appears in the captured
frames. Leaving the activity or stopping capture cancels the sequence. A
dedicated **Cancel usage check** button is available while it runs; using the
test tone control also cancels the sequence and starts or stops the manual tone.
This checks only SoniMesh's own playback under those usage tags; it does not
predict another app's capture behavior. Copying or sharing capture evidence
includes the most recently completed usage check result from that capture
session. The completed result is retained through activity recreation; an
in-progress sequence is canceled when the activity leaves the foreground.
The local observations also show the highest PCM sample magnitude seen during
the current run as a percentage of full scale. It helps distinguish a very weak
signal from silence, but is not an audible loudness or quality measurement. The
status card displays that run peak as a normalized bar and percentage; it is a
maximum-to-date reading, not an instantaneous level meter.
On 2026-11-25, API 37 x86_64 emulator instrumentation passed the built-in tone
capture/stop/restart/activity-recreation flow, the active-session activity-
reopen/source-retention flow, and capture of each supported usage in one
session. The scroll helper searches upward first for stop controls after
locating lower source controls, and includes the visible text snapshot when it
cannot find a control. Earlier attempts hit an Android System UI
not-responding dialog; dismissing it and allowing the emulator to settle cleared
that transient failure. These results do not establish audible output or
physical-device behavior. A follow-up instrumentation run also found the
state-card peak label and matching nonzero percentage while the tone was
captured.
On 2026-11-23, an API 37 x86_64 emulator accepted **All supported** and began
capturing after fresh consent; the screen showed the selected usage, increasing
silent-frame counts, and zero read errors. No playback source was exercised in
that run, so it does not verify signal capture for each usage or on a physical
device.

For the device-side all-usage check, use an isolated English-language API 37
emulator with no other audio playing. From `apps/android`, build and install
`:app:assembleDebug` and `:app:assembleDebugAndroidTest`, grant `RECORD_AUDIO`
and `POST_NOTIFICATIONS` to `com.sonimesh.sender`, then run:

~~~powershell
adb shell am instrument -r -w -e class com.sonimesh.sender.AllUsagesCaptureTest com.sonimesh.sender.test/androidx.test.runner.AndroidJUnitRunner
~~~

The test grants fresh projection consent through the system UI and plays a
generated tone for each usage in one capture session. It waits for silence
between tones and checks for new non-silent captured frames. It also starts
immediately after selecting the usage to check that the selection reaches the
service without waiting for a redraw. On 2026-11-23 it
passed twice on an API 37 x86_64 emulator; this is not a physical-device or
source-app compatibility claim. CI builds the test APK but does not run it.
On 2026-11-24, the same emulator test also checked that the capture notification
changes between no capturable audio and observed signal for each usage. The
notification reports a signal observation, not the cause of silence.
On 2026-11-25, the API 37 x86_64 emulator also passed
`notification_stop_action_ends_capture_from_background`: after moving the
activity to the Home screen, the foreground notification's Stop action ended the
capture and removed the notification. This verifies the emulator service and
notification lifecycle only; it is not evidence for physical-device background
execution or vendor power-management behavior.
The same API 37 AVD passed `capture_remains_active_while_screen_is_off` with a
swipe keyguard enabled: `KeyguardManager.isKeyguardLocked` and
`PowerManager.isInteractive == false` were both observed while capture and its
foreground notification remained active; after wake and keyguard dismissal, the
activity still stopped capture normally. The stock AVD had its lock screen
disabled, so the test setup used `adb shell locksettings set-disabled false`.
Record the original value from `adb shell locksettings get-disabled` and restore
it after the run; the default AVD value was restored with
`adb shell locksettings set-disabled true`. This emulator run does not establish
physical-device lock-screen behavior or battery impact.
To repeat the lock-screen row, select the API 37 AVD and record its current
lock-screen setting. If `adb shell locksettings get-disabled` prints `true`,
enable the swipe keyguard for the test:

~~~powershell
adb shell locksettings set-disabled false
adb shell am instrument -r -w -e class com.sonimesh.sender.AllUsagesCaptureTest#capture_remains_active_while_screen_is_off com.sonimesh.sender.test/androidx.test.runner.AndroidJUnitRunner
adb shell locksettings set-disabled true
~~~

Restore the recorded setting rather than blindly applying the last command when
the AVD was already configured with a keyguard.

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
The desktop output probe provides **Copy invitation** and **Share invitation**
beside the displayed payload. Sharing uses the system share API when available
and otherwise falls back to copying; if both are unavailable, the read-only
payload can still be selected manually.
The Android lab's **Paste lab invitation** button reads the current clipboard,
rejects empty or oversized text, and fills the invitation field for review before
the operator applies it.
It also accepts a shared `text/plain` invitation from the Android share sheet and
fills the same field without starting capture automatically. Both paths reject
empty, oversized, or non-hexadecimal text before filling the field.
For a LAN where both routes use the same address, **Apply invitation and use
address for UDP** also fills the UDP destination. A loopback invitation leaves
the UDP destination unchanged for ADB reverse; enter the emulator host alias
separately. The payload contains the fixed public lab fixture secret and is not a production QR or a
persistent trust record. When control and UDP use the same LAN address, **Use
control address for UDP** copies it explicitly; loopback is rejected because
ADB reverse does not forward UDP. Reopening the activity keeps the active capture
or failure status instead of replacing it with the saved-setup reminder. If using
the same lab invitation for a retry, the activity retains its complete payload;
editing its SPKI or control host switches to manual setup and clears the saved
invitation. This reuse applies to the public lab fixture, not production pairing.
If using the older manual setup, copy the printed
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

To check another capture after stopping, start the listener with
`--keep-listening --require-tone` and run the same command with the test method
`captures_again_after_stopping`. It starts and stops two capture sessions in
separate activity launches, granting fresh screen-capture consent each time.
On 2026-11-23, an API 37 x86_64 emulator using the manual SPKI, ADB-reversed
control, and emulator UDP decoded 154 frames in each session with nonzero PCM
peaks; this is software evidence, not a physical WLAN, audible playback, or
latency measurement.

To test the paired media path with controlled UDP impairments on the desktop,
start the listener and relay in separate terminals from the repository root:

~~~powershell
cargo run -p opus-codec-prototype --bin paired-audio-lab -- --media-port 39094 --status --require-tone
cargo run -p sonimesh-network-simulator --bin udp-impairment-relay -- --seconds 120 --seed 17 --loss 10 --delay-ms 20 --jitter-ms 5 --reorder 10 --reorder-delay-ms 30
~~~

Run the instrumentation command above while both are running. The sender defaults
to UDP port 39093; the relay forwards to the listener on 39094. Defaults bind
only to loopback. For a physical sender, use the listener's existing `--bind`
and `--host` options and bind the relay to the intended interface with
`--listen IPv4:39093`. The relay accepts at most 1,200 bytes per datagram, queues
at most 256 copies by default, and reports its profile, drops, forwarded copies,
and copies still queued when its configured duration ends. Queued copies are discarded at
exit; choose a duration that covers setup, capture, and drain. Its seed fixes
model decisions for the received packet sequence, but actual arrival and delivery
timing depend on the host and OS. The control channel is not impaired.
Use `--seconds 28800` for an eight-hour run; the duration may be set from one
second to 24 hours. This option alone does not establish a completed soak.
Add `--report-seconds 5` to print cumulative relay counters every five seconds
while it runs. Periodic reports are disabled by default; output can affect live
scheduling, and counters do not establish desktop receipt or playback.
Reports also include cumulative in-limit UDP payload bytes (`received_bytes`,
including packets later dropped by the model),
payload bytes sent by the relay (`forwarded_bytes`, including duplicates), and
the highest number of copies queued at once (`peak_queued`). Oversized datagrams
are excluded from the byte counts. Compare byte-counter differences over the
reported elapsed time to estimate traffic rates; socket sends do not prove receipt.
For repeated short outages, add `--burst-interval 50 --burst-length 4` to
drop four consecutive submitted packets after the first 50 and again every 50
submissions thereafter. The interval counts packets received by the relay, not
time; the length must not exceed the interval. Both default to zero (no burst).
For a constrained-link experiment, add `--bandwidth-bytes-per-second 12000`
and `--queue-packets 32`. The bandwidth counts UDP payload bytes, including the
protected media header and authentication tag, but excludes UDP, IP, and link-layer
headers. Zero bandwidth (the default) disables shaping; the supported range is
0 to 1,000,000,000 bytes per second. The queue limit counts scheduled copies,
including duplicates, and ranges from 1 to 4096 (default 256). Once full, the
relay drops additional copies and reports them in `capacity_dropped_copies`.
The model rounds each serialized copy up to whole milliseconds; the live host's
scheduling and socket buffers also affect the observed result.

For a desktop-only software check without Android, run the relay with loopback
listen port 39093 and forward port 39094, then run
`cargo run -p opus-codec-prototype --bin local-audio -- --check-via-relay 39093 39094`.
This sends a generated Opus tone through the protected UDP path and checks the
decoded software render. It uses a fixed test key and binds both endpoints to
loopback; it does not test Android capture, authenticated pairing, or audible
output. Loss or excessive delay can make this strict check fail. It waits for
each frame before sending the next, so it does not reproduce a continuously
paced sender or by itself test congestion. On 2026-11-24 a Windows software
run with 12,000 bytes/second and a 32-copy queue decoded 100 frames and observed
a nonzero software-render peak; the relay reported 100 shaped copies, 30,159 payload bytes, and
no queue-capacity drops. This is not Android or physical WLAN evidence.

To exercise a paced software stream, start these commands in separate terminals
from the repository root. The relay runs long enough to cover the sender's
stream and its final playout ticks. The sender defaults to two seconds; pass
`--seconds N` to choose a run from one second to one hour. Set the relay duration
longer than the sender run so the final frames can drain:

~~~powershell
cargo run -p sonimesh-network-simulator --bin udp-impairment-relay -- --listen 127.0.0.1:39093 --forward 127.0.0.1:39094 --seconds 15 --loss 5 --delay-ms 20 --jitter-ms 5 --queue-packets 32
cargo run -p opus-codec-prototype --bin local-audio -- --stream-via-relay 39093 39094 --seconds 10
~~~

The paced sender emits one 20 ms frame at a time. Its receiver reports admitted,
decoded, concealed, and empty playout ticks, and the command fails if the software
render is silent. Relay and receiver summaries describe the software path only;
live host scheduling is not deterministic timing evidence.
On 2026-11-24, a Windows loopback run with 5% configured loss, 20 ms base delay,
and 5 ms jitter decoded 94 frames and concealed 6, matching the relay's 94
forwarded copies. With the same profile plus a 12,000-byte/second limit and a
32-copy queue, the relay forwarded 94 copies by shutdown while the two-second
playout had decoded 1 frame and concealed 99; its peak modeled queue was 18.
That run shows the configured rate cannot sustain this prototype's encoded
stream with a 60 ms startup buffer. Neither run measures physical network timing
or listening quality.

The desktop output probe can instead start the playback-enabled listener: set
its **Receiver UDP port** to 39094 and run the relay separately. Restore the
default port 39093 for a direct connection. Android's **Desktop UDP port** is
independent and defaults to 39093; set it to the relay's listen port or the
listener's media port for a direct connection. Changing either setting does not
configure the other endpoint.
The probe shows the running listener's configured UDP port separately from the
settings for its next start, including after the probe view is reopened. This
is the requested port, not a connectivity or successful-bind check.
For the paired capture instrumentation, pass `-e mediaPort 39094` when testing
a direct listener started with `--media-port 39094`; omit it for the default.
This argument applies to `PairedCaptureTest`, which drives the interactive lab
service, and to `PairedAudioInteropTest`, which sends synthetic Opus directly.
On 2026-11-24 an API 37 x86_64 emulator used port 39094 for a direct paired
capture; the listener authenticated and decoded 155 packets with a software PCM
peak of 0.094. This does not verify physical WLAN or audible playback.

On 2026-11-24, one API 37 x86_64 emulator run sent 100 encoded packets through
the example profile. The relay reported 14 random losses and 12 packets given
extra reorder delay; the listener authenticated 86 packets, decoded 75 frames,
and reported a 0.249 software PCM peak. This validates the software lab path,
not physical WLAN, playback quality, or acoustic latency.

Another API 37 x86_64 emulator run on 2026-11-24 used `--burst-interval 20
--burst-length 4 --delay-ms 0 --jitter-ms 0` with the same capture test. The
relay received 156 packets and dropped 28 in bursts; the listener authenticated
and decoded 128 packets, reported 31 PLC decisions and a 0.098 software PCM
peak. This exercises repeated interruption on the software lab path, not
physical WLAN, audible quality, or acoustic latency.

The test synthesizes and encodes a tone; the desktop acknowledges start only
after binding its UDP receiver with the pairing-derived root and acknowledges
stop after retiring it. Successful runs report the decoded frame count and
software PCM peak. During live playout, the paired lab uses Opus packet-loss
concealment for missing frames, including when the jitter buffer is empty. After
three consecutive concealed frames and an empty buffer it waits for fresh media
to re-establish playout. This is a fixed lab policy, not adaptive jitter control.
On stop, the paired lab briefly polls for trailing UDP and
plays out buffered frames before closing the receiver, with a fixed upper time
limit. UDP does not identify the final packet, so this is best effort and does
not guarantee that every frame or hardware output buffer has played. On
2026-11-22, two API 37 x86_64 emulator runs decoded 100
and 92 frames respectively, each with a peak of 0.250; the discrepancy has not
been characterized. This is not playback-capture, physical WLAN, listening,
latency, persistent trust, or production UI evidence. The interactive audio lab
also has a **Start paired UDP capture** mode: start `paired-audio-lab` without
`--require-tone`, paste its printed SPKI_HEX in the app, select that mode,
grant capture consent, and play a capturable source or use **Start test tone**.
The lab's capture usage selector applies to the next capture session: **All**
includes media, game and unknown usage, while the other choices select one
usage. The test tone is a looping 1 kHz signal at either 48 kHz or 44.1 kHz,
depending on the output formats accepted by the device. A new test tone uses
the current capture session's usage while the service is starting, running, or
stopping; otherwise it uses the selector.
**All** uses Media. Changing the selector does not change a tone already
playing: stop and restart the tone to apply the applicable usage. On
the interactive lab screen, **Copy lab evidence** copies a bounded text summary
of the current status, session request, sender queue drops, sent packet count,
encoded payload average, and the latest capture signal state. For UDP sessions,
the screen and evidence also show the requested destination port from the
current or most recent session, independently of the next-session field. This
does not confirm desktop receipt. On 2026-11-24 an API 37 x86_64 emulator
displayed port 39094 during paired capture; the listener authenticated and
decoded 174 packets with a software PCM peak of 0.093. This is software-lab
evidence, not physical WLAN or audible playback. The signal state
is **no capturable audio** after a bounded silent interval, **signal detected**
after non-zero PCM arrives, or unknown before the capture has produced enough
samples. The same evidence includes elapsed capture duration in seconds for the
current or most recent capture. It excludes the invitation, SPKI, and audio
data.
**Share lab evidence** sends that same text summary through the Android share
sheet; if no share target is installed, the activity reports that locally.
The displayed status message is bounded before it reaches the screen or the
evidence summary, so an unexpectedly long listener error cannot expand the
operator report without limit.
After a valid invitation is applied, the lab keeps the invitation, desktop SPKI,
control address, and last UDP address in the app's private preferences so the
next activity launch can resume the same controlled-lab setup. The screen shows
that the setup was restored and provides **Clear saved lab setup** to remove
those values and reset the address fields. This convenience is for the public
lab fixture; it is not a persistent product trust store.
On 2026-11-23, an API 37 x86_64 emulator test checked all four
choices and confirmed the playing tone retains its usage across activity
recreation; this checks audio attributes, not audible output. Android and the source application still
control whether playback can be captured; the selector does not select apps.
The paired-capture instrumentation method `tone_follows_active_capture_usage`
also exercises a Media capture while the next selection changes to Game. It
waits for silence, recreates the activity, restarts the tone, and checks that
the capture detects signal again before testing the selection after stop.
On 2026-11-23, this test passed on an API 37 x86_64 emulator using ADB-reversed
control and emulator UDP; the software listener decoded 276 frames with a
cumulative peak of 0.102. The signal transition is observed on Android; the
listener's cumulative peak does not isolate the restarted tone. This does not
establish physical WLAN, device playback, or audible output.
An invitation pasted into the lab but not yet applied remains in the field if
Android recreates the activity, so it can still be reviewed and applied.
The lab also lets the operator request 64, 96 (default), or 128 kbps Opus for
the next capture. This is a MediaCodec target, not a measured or guaranteed
bitrate; changing the selector does not reconfigure an active capture. The
usage and bitrate selectors are retained in the app's private preferences, so
the next lab launch keeps the operator's last choices; they still only apply
when a new capture starts.
When a paired TCP connection is refused during startup, the service retries up
to five times at one-second intervals while retaining the current projection.
The status reports the wait, and Stop cancels it. Other connection failures are
reported without this startup retry. After an established paired session drops,
the experimental sender keeps its current projection, AudioRecord, and encoder
running while it retries a new paired media session with a 500 ms to 4 s bounded
backoff. Encoded packets from the retired session are discarded before sending
under the new session. Twelve unsuccessful reconnect attempts end capture with
an actionable status; Stop cancels an in-progress retry. Automatic recovery
requires the desktop lab to stay running with `--keep-listening`; otherwise the
retry eventually ends. This behavior uses the public fixed lab fixture and is
not the product pairing or reconnect policy.
While reconnecting, the connection status remains visible even if the capture
signal changes; the separate capture-signal indicator continues to update.

To verify one established-session interruption on an API 37 emulator, start the
listener with a one-time control disconnect after five seconds:

~~~powershell
adb reverse tcp:47610 tcp:47610
cargo run -p opus-codec-prototype --bin paired-audio-lab -- --bind 0.0.0.0 --host 10.0.2.2 --keep-listening --disconnect-after-ms 5000 --require-tone --status
~~~

Use the printed `INVITATION_HEX` and run the instrumentation method below. The
listener closes only the first control session, then accepts the Android
reconnect using the same public lab invitation. The test checks that capture
duration continues and the service remains active without a second projection
consent. This is emulator loopback evidence, not physical WLAN or audible
continuity evidence.

~~~powershell
adb shell am instrument -w -e pairingInvitationHex $invitationHex -e mediaHost 10.0.2.2 -e reconnectTest true -e class 'com.sonimesh.opusspike.PairedCaptureTest#reconnects_after_desktop_disconnect_without_restarting_capture' com.sonimesh.opusspike.test/androidx.test.runner.AndroidJUnitRunner
~~~

To check cancellation during the reconnect window, use the same listener and
run `PairedCaptureTest#stops_capture_during_reconnect_without_restarting_session`
with `-e reconnectTest true`. The test presses Stop after the first disconnect
is reported and checks service/admission cleanup and paired worker termination.
On 2026-11-25 this passed on an API 37 x86_64 emulator in 16 seconds. This is
loopback lifecycle evidence only.

On a normal paired stop, the capture worker is interrupted and the supervisor
gets time to drain media and request a desktop StopStream confirmation. If it is
still blocked, a background task closes the TLS socket after 3.5 seconds; TLS
socket close is kept off the service main thread because Conscrypt may write a
close-notify record. Each stop reply read has a two-second timeout, which is not
a deadline for the entire shutdown. The UI reports when the desktop did not
confirm the stop.
On 2026-11-24 an API 37 x86_64 emulator observed the retry, then completed a
paired capture after a loopback control gate opened. The software listener
authenticated 147 packets and decoded 124 frames with a peak of 0.391; it also
reported late packets and buffer resynchronizations. This does not establish
physical WLAN recovery or audible continuity.
On 2026-11-25 an API 37 x86_64 emulator passed the reconnect-and-stop test: after
the injected first-session disconnect, the listener accepted a fresh paired
session and the Android test confirmed media packet counts continued increasing
after recovery. The listener completed the session after StopStream; the Android
service exited, capture admission reset, and paired control/media worker threads
terminated. The ordinary paired-stop control test also passed. This verifies
emulator loopback service lifecycle, not physical WLAN recovery or audible
continuity.
During paired reconnects, the foreground notification now reports connecting,
reconnecting, and restored states alongside transport and mute state. On
2026-11-25, API 37 x86_64 instrumentation moved the activity to the background
before the injected disconnect, observed the reconnecting notification, returned
through its open action, and verified the restored notification while capture
continued. The loopback listener received 188 packets and decoded 178 frames
with a cumulative peak of 0.436. This is emulator service and decoder evidence,
not physical WLAN or audible continuity evidence.
On 2026-11-25, the API 37 x86_64 startup-refusal test also verified that the
connecting notification clears after the first session starts; the listener
received and decoded 172 packets. The established-session reconnect and mute
notification tests passed separately. These are emulator loopback checks.

To verify recovery after automatic reconnect exhaustion, run the listener with
`--keep-listening --disconnect-after-ms 1000 --reject-starts 33` and invoke
`PairedCaptureTest#retries_capture_after_reconnect_attempts_are_exhausted` with
`-e recoveryTest true`. The initial disconnect counts as the first of the 12
failures, leaving 11 paired reconnect sessions to exercise the listener's three
StreamStart retries each. The test then selects fresh screen-capture consent
through Android's system dialog and checks that manual Retry starts a new
capture. On 2026-11-25 this passed on an API 37 x86_64 emulator; the desktop
listener accepted the manual session, received nine media packets, and decoded
nine frames with a cumulative peak of 0.094. This is loopback recovery and
decoder evidence, not audible playback or physical WLAN evidence.
On the same emulator, a later run stopped and restarted the test tone during
reconnect, observed both signal transitions while the reconnect status remained
visible, and then successfully retried after exhaustion. Separate normal-signal
and reconnect tests passed as well.

To repeat the earlier reconnect-through-gate test, start these commands in
separate terminals from the repository root (leave ADB reverse for port 47610
disabled):

~~~powershell
cargo run -p opus-codec-prototype --bin paired-audio-lab -- --bind 127.0.0.2 --host 10.0.2.2 --media-port 39094 --require-tone --status
cargo run -p sonimesh-network-simulator --bin udp-impairment-relay -- --forward 127.0.0.2:39094 --seconds 120 --delay-ms 0 --jitter-ms 0
python tools/fixtures/paired_retry_gate.py
~~~

Grant the lab's audio and notification permissions, then run the following
instrumentation with the listener's `INVITATION_HEX` in `$invitationHex`; select the entire
screen in the system consent UI:

~~~powershell
adb shell am instrument -w -e pairingInvitationHex $invitationHex -e mediaHost 10.0.2.2 -e retryGate true -e class 'com.sonimesh.opusspike.PairedCaptureTest#captures_after_refused_connection_is_retried' com.sonimesh.opusspike.test/androidx.test.runner.AndroidJUnitRunner
~~~

The test sends a local UDP marker only after observing the retry status; the
gate then accepts one control connection and forwards it to the listener. This
is a test fixture, not a discovery or production reconnection mechanism.
The lab displays the usage, target bitrate, and transport requested for the
current or most recent session separately from the selectors for the next
session. Transport is shown as **ADB TCP**, **UDP**, or **paired UDP** so an
operator can distinguish the bootstrap path while diagnosing a run. This is the
requested configuration, not confirmation that capture or encoding succeeded;
it remains until the next service start and is lost if the app process ends.
On 2026-11-23, an API 37 x86_64 emulator paired-capture test requested Media
at 128 kbps, changed the next usage to Game, and recreated the activity; the
session request stayed visible and the software listener decoded 277 frames.
A separate UI test checked that the request remains after service stop. These
checks do not establish physical WLAN, audible output, or actual encoder bitrate.
**Encoded payload average** shows cumulative Opus bytes divided by elapsed
capture time, including encoder tail drain and packets later discarded locally.
It becomes available after one second, excludes packet and network overhead,
and is retained until the next capture; it is not network throughput.
On 2026-11-23, separate approximately three-second API 37 x86_64 emulator tone
captures reported 60 kbps for a 64 kbps target and 121 kbps for a 128 kbps
target. These short-run encoded-payload averages include startup and tail time;
they are not physical-device, network-throughput, or audio-quality measurements.
On 2026-11-23, separate API 37 x86_64 emulator paired-capture runs selected 64
and 128 kbps and decoded 154 and 150 frames respectively on the software
listener, with peaks 0.100 and 0.093. These are interoperability checks, not
actual bitrate, quality, physical-device, or latency measurements.
**Sender queue drops** counts encoded packets discarded locally when the bounded
sender queue is full, for the current or most recent capture. It does not count
network loss or confirm playback; it resets when the next capture starts.
On 2026-11-23 an API 37 x86_64 emulator run verified that the count is visible
during paired capture and remains visible after stop; the software listener
decoded 151 frames with a 0.094 peak. A separate recreation test restored a
synthetic count. Neither run injected sender-queue congestion.
**Media packets sent** counts packets successfully written to the selected TCP
or UDP sender. Compare it with sender queue drops when diagnosing the lab; it
does not prove network delivery or desktop receipt and is retained after stop.
The lab foreground service holds a partial CPU wake lock until the service is
destroyed so it can be evaluated with the screen off. Physical-device screen-off
continuity and power consumption remain unmeasured.
During an active lab capture, **Mute sending** replaces subsequent encoder input
with silence while capture, the connection, and capture-signal observation continue.
The activity places **Stop capture** and **Mute sending** near the session status,
ahead of the connection setup fields, for use during an active session.
The **Start test tone**/**Stop test tone** control is in the same area so an
operator can create or stop a known capturable source without scrolling.
If the selected output cannot create an `AudioTrack`, the lab keeps the control
ready for another attempt and reports that the test tone is unavailable on the
device instead of crashing the activity. It tries common 48 kHz and 44.1 kHz
stereo/mono formats before reporting that failure.
After a capture service finishes, including after a desktop connection or capture
failure, it also shows **Retry last capture** near the same status area. The
button appears after cleanup completes. The retry reuses the last service's
transport even if the activity was closed and reopened in the same process. It
uses the current setup fields, then requests any required permission or
screen-capture consent again;
starting a new session hides the retry action.
Unmute resumes captured audio without a new consent session. Audio already encoded,
in transit, or buffered at the receiver can still play briefly after muting; this
control is not an immediate stop. It resets when the session ends.
The foreground notification also offers **Mute sending** or **Unmute sending**;
its action and the activity switch reflect the same service-owned state.
While muted, its transport text also says **Sending muted** and returns to the
normal transport text after unmuting.
On 2026-11-24 an API 37 emulator invoked these notification actions while paired
with the software listener and checked synchronization with the activity switch,
including activity recreation. The listener observed peaks near 0.093 before
muting, zero across six reported intervals while muted, and about 0.094 after
unmuting; it reported 654 packets, 7 late packets and 8 resync drops. A separate
test returned to capture from the notification. These checks invoked notification
intents programmatically and do not establish drawer presentation, physical WLAN,
or audible playback.
On 2026-11-24, an API 37 emulator paired with the software listener, continued
sending packets while muted, retained the switch state after activity recreation,
and unmuted and stopped. With `--signal-status`, the listener reported decoded
peaks of 0.094–0.102 before mute, `0.000000` across five one-second windows of
50 decoded frames while muted, and about 0.094 after unmute. A host test also
checks selection of the silent encoder input. These observations do not establish
physical-device behavior or audible output.
On Android 13 and newer the lab requests notification permission before screen
capture consent, so the capture notification and its stop action can appear in
the notification drawer. If permission is denied, enable it in app settings
and retry; capture does not start. The notification opens the lab activity.
Its body identifies the selected transport, and its subtext updates once per
second with the elapsed capture duration while the service is running; the
duration remains visible after leaving the activity until the service stops.
On 2026-11-23, an API 37 emulator rejected notification permission, retried and
granted it, then reached screen-capture consent. A separate paired capture
granted permission during startup, returned through the notification, and decoded 182 frames in
the software listener; this does not verify physical notification presentation.
On 2026-11-23, an API 37 x86_64 emulator test returned home during paired
capture, invoked the published notification's content action, observed the
active capture screen, and stopped normally; the software listener decoded
163 frames with a 0.094 peak. The test invoked the notification action directly;
notification-shade presentation and physical-device behavior remain unverified.
For an emulator check, run the paired capture instrumentation with the method
`keeps_service_active_while_screen_is_off` and the listener's `--status` option.
The test checks that the screen remains off and the service remains present;
check listener status separately for media progress. On 2026-11-23 an API 37
x86_64 emulator with ADB-reversed control and emulator UDP remained noninteractive
for eight seconds; a listener update during that interval reported 224 decoded
frames and a 0.095 PCM peak. The system power log showed the lab wake lock being
acquired and released after stop. This is not a physical-device, power, listening,
or latency result.
When the encoder temporarily cannot accept a complete capture frame, the lab
retains it and continues polling output. It pauses briefly if neither side can
progress; after two seconds without accepting that frame, it stops the session
and asks the user to retry. Capture continuity during codec stalls is not
guaranteed because the platform capture buffer can overflow.
If the desktop disappears during capture, the lab stops when it observes a
control-channel closure or a UDP send failure. The first observed failure is
shown; check the listener and network, then start a fresh capture. UDP send
success alone does not prove that the desktop is reachable. On 2026-11-23 an
API 37 x86_64 emulator reported a UDP send failure and stopped after the desktop
listener process was terminated; a subsequent separate instrumentation run with
a restarted listener, its new SPKI, and fresh consent decoded 153 frames with a
nonzero PCM peak. This does not demonstrate automatic reconnect or physical WLAN.
**Stop capture** requests the desktop to stop the stream. This mode uses the
same fixed public lab invitation and ADB-reversed control connection. The
Android sender first gives its encoder a bounded opportunity to flush pending
packets, padding a partial final PCM frame with silence, and then gives its
bounded UDP queue a short time to drain before requesting stop. The waits and
desktop tail drain are best effort, so
network loss or slow shutdown can still truncate audio. The Android encoder tail
drain continues its bounded wait when the capture worker
has been interrupted for stop, and restores the thread's interrupt status after
the drain; this does not guarantee delivery of the final packet.
The foreground service stays in a stopping state until this cleanup completes;
repeated start requests during that interval are ignored. A stalled control
reply can delay completion until the control command times out. The Android
lab reports when capture has stopped locally but the desktop did not confirm
the stop; in that case check or restart the listener before trying again.
The isolated codec tail check can be run after installing the lab and test APKs:

~~~powershell
adb shell am instrument -r -w -e class com.sonimesh.opusspike.EncoderTailDrainInstrumentedTest com.sonimesh.opusspike.test/androidx.test.runner.AndroidJUnitRunner
~~~

On 2026-11-23 this passed on an API 37 x86_64 emulator: the tail-drain helper
observed encoder end-of-stream and at least one audio packet. It did not check
the final packet's content, the capture service, network delivery, or a physical
device's codec. On 2026-11-25 the helper also reached end-of-stream with a
pending thread interrupt on an API 37 x86_64 emulator; this was an isolated codec test.
The lab activity also ignores repeated start requests while permission or screen
capture consent is pending, or while its service is active or stopping. It shows
a brief message without replacing the current session status. Once
the service finishes, starting again requests fresh screen-capture consent.
If **Stop capture** is used while a permission or consent request is pending,
the lab cancels that start and ignores its eventual result. Close the system
dialog before starting another capture.
Copied or shared Android lab evidence includes the last known sending-mute
state. It is retained after a session stops and starts off for each new session;
it does not record the full mute history.
The desktop defaults to a software PCM check. On Windows, add `--features playback`
before `--` and pass `--play` after it to use the default output device; add
`--select-output` to choose a device. `--require-tone` checks decoded audio,
not whether it was audible. Output callback counts are not a listening test.
For controlled network experiments, `--startup-buffer-ms` selects the initial
wait after the first packet, in 20 ms steps from 20 to 120 ms (default 60 ms).
The same wait is used after output recovery or decoder resynchronization. This is
a fixed startup delay, not an adaptive jitter target or an end-to-end latency
measurement; a longer wait can overflow the bounded packet buffer under bursts.
After a delayed playout decision the lab skips missed frame deadlines and waits
at least 10 ms before another decision. This bounds catch-up pacing; it does not
prevent loss caused by a host stall or establish physical audio continuity.
On a normal stop, `RESULT schedule` reports the number of live playout
decisions, decisions at least one frame late, and the maximum lateness in
microseconds. These measure the lab loop against its own deadlines, exclude
the stop drain, and do not measure packet arrival time or physical playback.
On normal stop, `RESULT resync` reports up to the first eight live jitter-buffer
resynchronizations: the number of buffered frames discarded, the interval since
the preceding receive-loop iteration, and the age and lateness of the most recent
playout decision when available. A total event count indicates omitted events.
These observations are captured in memory and printed after the stop drain; they
exclude resynchronizations during that drain. Loop intervals are not packet
arrival times, and a nearby delay does not establish why a resynchronization
occurred.
On 2026-11-23, two separate API 37 x86_64 emulator sessions exercised the new
report with ADB-reversed control and software output. The notification-return
test decoded 192 frames, with zero resynchronizations and no frame-late
decisions among 196 (maximum lateness 13,208 microseconds). The screen-off test
decoded 408 frames, with zero resynchronizations, two late packets, and no
frame-late decisions among 414 (maximum lateness 7,187 microseconds). Both
instrumentation tests passed. These runs verify the zero-event report only;
they do not verify nonzero event details, explain earlier drops, or establish
physical WLAN or audible playback behavior.
In one 2026-11-23 API 37 emulator software session, the notification-return
capture test completed with 193 live decisions, one at least 20 ms late, and
26,122 microseconds maximum lateness; the listener decoded 177 frames and
reported 5 late packets and 8 resynchronization drops. This single uncontrolled
run confirms the observation path, but does not correlate the late decision
with the drops or identify their cause.
On 2026-11-23, separate API 37 emulator sessions completed with the revised
schedule: fixed mode decoded 199 frames with 8 resynchronization drops;
adaptive mode decoded 160 frames with 16 drops. Both reported zero late packets.
These short, uncontrolled software runs do not establish a before/after
improvement or identify the causes of the remaining drops.
Add `--signal-status` for one-second decoded-frame counts and peak amplitude
for each interval; an interval without decoded frames reports `peak none`.
This signal observation is before output gain and excludes packet-loss concealment;
it does not establish audible playback. The existing cumulative status is unchanged.
Add `--status` to print packet counts, decode counts and peak, jitter occupancy,
and output recovery state at session start and every five seconds afterward. This can help
distinguish missing media from decoded silence; counts and peak are cumulative
for the session, and they do not prove audible output. On a normal stop, the
listener emits a final snapshot after its tail drain. The desktop probe shows
late-packet and jitter-resynchronization dropped-frame counts alongside the other
observations; these identify different causes of missing frames, not the cause
of network delay. An older listener omits these values and the probe hides them.
The media-arrival observation distinguishes waiting for the first accepted
packet, a packet received within the last two seconds, and a longer gap after
reception. State changes are reported as the listener observes them; the probe
polls its listener snapshot once per second. A gap can have several causes and does not by itself establish that
the sender, network, or listener is responsible. Older listeners omit it.
On a normal stop, the listener also reports the number and longest duration of
observed gaps of at least two seconds between increases in the authenticated
packet counter. The listener polls that counter, so these are not exact packet
arrival timestamps.
The final gap ends when the stop snapshot is taken, after the tail drain; time
before the first packet is excluded. These are listener observations, not a
measurement of audible interruptions, and abnormal termination has no final
summary.
On 2026-11-23, an API 37 emulator and software listener reported 7 late packets
and 24 resynchronization drops in both the final status and result; 174 frames
were decoded. This does not characterize physical WLAN or audible playback.
The probe retains the most recent session's observations while waiting; if a session fails before
a final snapshot, it labels the retained values as the last update. A new client
attempt clears the prior observations. When its view reloads, the probe checks
for an existing listener and resumes its status updates. Settings shown in a
restored view apply to the next start and may differ from the running listener.
The probe stores the host fields, selected output index, startup buffer, and
fixed/adaptive choice in browser storage so a page reload keeps the operator's
last lab setup. Values are length- and enum-bounded; malformed storage is
ignored, and no invitation or session secret is stored there.
Use **Reset lab settings** to remove that browser entry and return the host,
output, buffer, and playout controls to their defaults without changing a
running listener.
If the listener process exits unexpectedly, the probe also retains the last
observations it had received; they can be incomplete. Starting a new listener or
stopping it explicitly clears them.
If a start or stop request fails, the probe checks the listener's current state
again; while that check is unavailable, it retries and keeps listener controls
disabled until the state is known.
Add `--control` alongside `--play` to adjust playback from the listener console:
enter `volume 0` through `volume 100`, `mute`, or `unmute`. These local settings
survive output recovery and successive sessions while the listener is running.
Silence can
reflect the source or platform capture policy. Other audio-lab modes below use
their separate ADB bootstrap. Remove the reverse mapping when no longer needed.

For a manually addressed controlled network, run the desktop listener with
`--bind <desktop-local-IPv4> --host <desktop-reachable-IPv4>` after `--`.
In the desktop output probe, entering the Android device's IPv4 address and
choosing **Suggest desktop address** asks the operating system which local
address it would route to that device. Review the suggestion before starting;
VPNs, routing changes, firewalls, or client isolation can still prevent access.
The suggestion does not send a packet or verify connectivity. Manual entry and
the separate ADB reverse configuration remain available.
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
session returns to listening. The desktop probe keeps a completion or failure
notice and any available observations while waiting for the next client. This
notice does not identify the cause of a failure. Stop the listener
with Ctrl+C. To exercise two
successive synthetic sessions, run the instrumentation method
`PairedAudioInteropTest#pairs_twice_with_continuous_listener` with the listener
in this mode and the same `pairingSpkiHex` and optional `pairingHost` arguments.
After an established connection drops, the paired lab attempts to reconnect
without ending capture or requesting projection consent again. Keep the desktop
listener running with `--keep-listening`; a listener that has exited cannot
accept the reconnect. The Android status reports recovery attempts and whether a
new paired media session was established.
Every listener status line is flushed as it is emitted, including `READY`,
`MEDIA`, `OUTPUT`, and periodic `STATUS` lines. A wrapper process can therefore
consume the listener's stdout as a live event stream instead of waiting for the
session to finish. The Tauri probe forwards those lines through a bounded
128-event channel and applies backpressure when snapshot polling is delayed, so
state transitions stay ordered instead of being silently dropped; the reader
exits when the probe closes the channel. The probe also retains the latest 32
`READY`, `RESULT`, `MEDIA`, and `OUTPUT` transitions in arrival order for quick
session diagnosis; periodic `STATUS` counters remain in the metrics view.

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
The synthetic paired-audio instrumentation accepts `-e mediaPort PORT` to send
to an alternate listener or relay port; it defaults to 39093, matching capture.
This only selects the destination and does not configure the relay or listener.

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

On 2026-11-24, the `resumes_after_media_pause` instrumentation test completed
against one continuously running listener after the listener moved its bounded
receiver and retry state off the control thread stack. The API 37 emulator sent
100 authenticated packets; the listener decoded 100, observed the intentional
media interruption and resumption, and reported six PLC frames with zero
rejections, late packets, or decode errors. This is a software/emulator loopback
check and does not qualify physical-network or audible-playback behavior.

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
source. **Start test tone** provides a local synthetic source; its button changes
to **Stop test tone** while it plays. The tone continues across activity
configuration recreation and is released when the activity finishes. **Stop capture**
or the notification action ends the stream. Restart the desktop listener for
each session. Remove the ADB mapping afterwards with `adb reverse --remove
tcp:39091`. Select the device with `adb -s SERIAL` if several are connected.

The service captures 48 kHz stereo PCM, encodes using the experimental
`:audio-encoder` MediaCodec module, and sends
length-prefixed Opus packets through an eight-packet queue (dropping the oldest
when full). Android connects to its own loopback and ADB forwards to a listener
bound to desktop loopback. The desktop decodes off the output callback and uses
the existing bounded PCM output source. A five-second read timeout ends a stalled
session. This is an unauthenticated ADB lab transport, not the product media
protocol or a WLAN receiver. Do not expose its port through a network tunnel.
Silent output can reflect source silence or platform capture policy; it does not
identify the cause. The lab currently lacks reconnection and device switching.
The encoder's device test can be run separately with
`./gradlew.bat :audio-encoder:connectedDebugAndroidTest` on an available device;
building its test APK alone does not exercise MediaCodec.

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
operator selects an endpoint. It can manage the separate lab listener but does
not change the system default output. If one endpoint's format cannot be read, it remains listed with an
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
shared workspace remains on Rust 1.85.0. Windows WebView2 is required. To use
its **Paired audio lab**, first build the child listener from the repository root:

~~~powershell
cargo build -p opus-codec-prototype --bin paired-audio-lab --features playback
~~~

The probe expects `target/debug/paired-audio-lab.exe` in this checkout. Start the
listener in the window and scan its lab invitation with the Android audio lab's
**Scan lab invitation** button. The built-in scanner requests camera permission;
copy and paste remains available if scanning cannot be used. The
experimental QR contains the same hexadecimal test payload as the text field;
it is not the production binary QR representation from ADR 0018. A scan from a
non-loopback address also fills the UDP destination; a loopback invitation keeps
the separately configured UDP destination for ADB reverse.
For a direct connection, the probe can optionally wrap the same lab invitation
with the running listener's UDP port as `SONIMESH-LAB1:<port>:<hex>`. Android
accepts this experimental text in scan, paste, and share flows and applies the
port along with the invitation and reports the applied port in the status. The
unwrapped hexadecimal invitation remains available, including for a relay whose
listen port differs from the receiver's.
During capture, setup feedback is shown separately so it does not replace the
active capture status. The updated fields apply to a later capture.
The paired request also snapshots its invitation before requesting permissions
and screen-capture consent, so changing the next setup during that flow does not
replace the invitation used by the pending request.
This wrapper is a lab convenience, not the production pairing QR format.
The probe recognizes numbered listener session events and keeps connection
endpoints out of its event history. To exercise the listener-to-snapshot path
with the API 37 emulator at `emulator-5556`, install both audio-lab debug APKs,
build the playback listener above, then run:

~~~powershell
cargo +1.98.1 test --manifest-path tools/tauri-output-prototype/Cargo.toml --locked emulator_capture_updates_real_listener_snapshot -- --ignored
~~~

On 2026-11-25 this test observed an active decoded signal and retained decoded
frame counts after the Android test stopped. It covers the Windows listener,
snapshot parser and emulator loopback, not the window interaction, audible
playback or a physical WLAN.
**Scan and start paired capture** applies a valid scanned invitation and then
requests the usual recording permission and screen-capture consent. A cancelled
scan or invalid invitation does not start capture. **Scan lab invitation**
continues to apply the invitation without starting, so the operator can review
or edit the addresses first.
For a pasted invitation, **Apply invitation and start paired capture** follows
the same one-step flow: a non-loopback control address also becomes the UDP
destination, while a loopback invitation retains the separate UDP address for
ADB reverse. Invalid input or an already active capture does not start a session.
The manual SPKI path also has **Paste desktop SPKI**, which validates the copied
hex key before filling the pairing field.
On 2026-11-23, an API 37 emulator test exercised the pasted-invitation flow;
the local software listener verified pairing and decoded 151 frames without
decode errors. This does not validate physical WLAN or audible playback.
On 2026-11-23, an API 37 emulator test injected a valid scanner result and
completed consent, pairing, capture, and stop; the software listener decoded
154 frames. A separate test rejected invalid scan contents before capture.
These checks did not exercise camera-based QR recognition or physical WLAN.
The lab's optional test tone continues across activity configuration recreation;
the button still stops it, and finishing the activity releases it. An API 37
emulator test on 2026-11-23 checked the AudioTrack play state and stop control
across recreation; it did not check audible output or physical-device behavior.
In a separate paired capture, the same emulator recreated the activity while
capturing a test tone. The active signal status remained after recreation and
the software listener decoded 309 frames with a 0.095 peak and no decode errors.
This checks emulator loopback rather than continuous physical playback or WLAN.
The control address defaults to `127.0.0.1` for ADB reverse; for a controlled LAN use an address Android can
reach. The listener binds TCP 47610 on all IPv4 interfaces and uses UDP 39093;
it plays through the selected output, or the default output when no device is
selected. Refreshing the device list retains an explicitly selected output when
the same endpoint remains available. If it disappears or its format is unavailable,
choose another output or the system default before starting the listener. This
reselection requirement persists when the probe view is reopened if local storage
is available; an unavailable selection does not silently become the system default.
Choosing the system default clears any previously saved explicit device selection.
The probe also honors a saved default when older settings retain a device description. On
2026-11-23, an explicitly selected endpoint remained selected after a refresh in
the Windows Tauri probe. This check did not cover device removal, reordered
devices, or audible playback.

Set the startup buffer before
starting the listener (20–120 ms in 20 ms steps, default 60 ms); it controls the
initial playout wait, not adaptive jitter or measured end-to-end latency.
The optional **Adaptive (experimental)** playout mode uses the Balanced target
and a bounded gate after that initial wait. Gated ticks can produce silence and
target decreases do not accelerate playback; compare it with the default fixed
mode before drawing conclusions. Neither mode proves physical latency or audio
quality.
On 2026-11-23, an API 37 emulator paired with the software-only listener in
adaptive mode and completed a short synthetic-tone capture: 151 packets and
151 decoded frames, with no rejected packets or PLC frames. This checks the
software path only; it does not establish WLAN or audible-playback behavior.
Stopping the listener terminates the
experiment immediately, including any active session; closing the probe also
stops its child. During an active session the probe shows the listener's
initial and five-second cumulative packet and decode counters, decoded peak, buffered frames,
and output recovery state. These are software observations, not proof of audible
playback. Media arrival and output recovery transitions are emitted promptly;
the cumulative counters retain their five-second cadence. The last observations
remain while waiting and after stopping the listener, and clear on the next
listener start or client attempt. Stopping the listener terminates an active
session immediately, so its retained counters are only the last available update,
not a final post-drain report.
Media arrival transitions received before the first cumulative status are kept
for that session and included when its first counters arrive.
On 2026-11-24, the Windows probe's ignored real-listener test sent 513 volume
commands without UI status polling and observed the last acknowledgement through
the background drain. Reproduce it after building the playback-enabled listener,
with TCP 47610 and UDP 39094 free:

```powershell
cargo +1.98.1 test --manifest-path tools/tauri-output-prototype/Cargo.toml real_listener_output_is_drained_without_ui_polling --locked --offline -- --ignored
```

This exercises the child process and control-output pipe without an active audio
session; it is not evidence of playback or sustained-load audio continuity.
During an active session, the probe also shows the last reported one-second
decoded-frame count and peak before output gain, excluding concealment. A zero
frame count means no frames were decoded in that interval, not decoded silence.
If no new signal line has been read for three seconds, the probe labels the
last observation as stale in both the window and copied or shared evidence. The
timer measures when the probe read the listener output, not when audio was
played; output buffering can delay observations.
This observation clears when the session ends and does not establish audible
playback. Rebuild the listener after updating the probe so it supports the
`--signal-status` option.
When observations are available, **Copy session evidence** copies a bounded text
summary of the displayed status, counters, active-session signal interval, and latest listener lifecycle
events without including the invitation or audio data. The listener retains the
latest 32 transitions and bounds each stored event; copied events are shortened
again to keep the operator report compact. If clipboard access is unavailable,
the visible metrics and events remain available for manual selection.
If a listener ends before producing metrics, the same action remains available
when lifecycle events exist and copies status plus those events so an early
startup or session failure can still be attached to a local report.
**Share session evidence** sends the same bounded summary through the browser
share API when the Tauri web view exposes it, and otherwise copies it to the
clipboard. A cancelled share leaves the current notice unchanged; neither path
includes the invitation or audio data. When no lifecycle events have arrived,
the evidence omits the empty event section.
For adaptive playout the observations also show the current target and range,
smoothed arrival jitter, gated ticks and forced playouts. These values describe
the current stream epoch: a decoder or jitter reset clears their history, while
the separate session counter tracks stream resets. A zero target means no adaptive
target has yet been published for that epoch. Session observations report the
actual playout mode independently of the next-start selection in the window.
In a second API 37 software-only capture on 2026-11-23, the final report showed
153 decoded frames, an adaptive target of 2 frames (range 2–4), 6 gated ticks,
and no forced playouts. This short emulator observation does not establish WLAN,
latency, or audio-quality behavior.
The probe's volume and mute controls apply to the lab receiver, including
successive sessions and output recovery while the listener remains running.
They do not change the system volume. The controls wait for the listener's
reported value before accepting another adjustment. If confirmation does not
arrive within five seconds, the probe allows a retry and reports that the result
is unknown; the displayed value remains the last one received from the listener.
On 2026-11-23 the Windows host's real-listener test selected its default output,
started the playback-enabled child, recovered from a deliberately incomplete
connection, observed the listener report volume 35 and muted, and stopped it.
The test also checked rejection of an unavailable output identifier. This covers
the host-to-child control path, not the window interaction or audible playback.
The probe prevents its test tone and listener from playing at the same time,
including across a page reload. Finish the test tone before starting the
listener, and stop the listener before testing an output. This does not control
audio from other applications.
This development-only UI uses
the public test invitation and does not establish product pairing, audibility,
or latency evidence.
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

## Android device checks

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
realtime callbacks. The Windows Tauri shell exposes a one-shot **Test output**
command backed by the selected CPAL endpoint; this verifies that the callback
rendered the bounded tone without proving that speakers are audible to a person.
The shell saves an explicitly selected output ID in its Tauri app configuration
directory and restores it at launch when it is still present. Missing or invalid
settings use the default endpoint; a missing selected endpoint falls back to the
current default or first available endpoint. This preference does not change
the Windows system default. **Follow system default** removes the saved explicit
selection; refresh the device list to pick up a later system-default change.
The saved preference remains intact during that fallback. After reconnecting the
preferred endpoint, use **Refresh output devices** to select it again.
The output inventory is cached between low-rate session snapshots. Use the
output panel's **Refresh output devices** control to re-enumerate endpoints after
connecting or removing a device.
If the initial device enumeration fails, the shell retains that failure until an
explicit retry instead of presenting an empty list as a successful enumeration.
The loading screen provides **Retry output devices** for this situation.
Low-rate snapshot polling has a separate UI state from user commands, so a slow
device inventory refresh does not disable output selection. An in-flight snapshot
is ignored after a newer user command starts, preventing stale state from replacing
the command result.
The shell-neutral `DesktopControlOwner` now provides the
low-rate command owner for lifecycle, output, stream, and diagnostics actions;
the live shell currently binds only output inventory, selection, and test tone.
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
owned pipeline. This remains a composition boundary separate from the current
output-setup shell; it is not a proof of Windows receiver audio output.
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
