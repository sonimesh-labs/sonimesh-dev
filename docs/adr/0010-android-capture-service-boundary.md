# ADR 0010: Android capture service and native boundary

- Status: Accepted design; capture implementation remains gated by device evidence
- Date: 2026-07-17

## Context

Phase 0A requires a capture-only Android prototype, but playback capture is
policy- and lifecycle-limited. Android 12-17 is the active validation range;
Android 14+ adds MediaProjection foreground-service and single-consent-session
requirements, while Android 17 local-network behavior needs its own discovery
validation. A UI-owned capture loop would be destroyed by ordinary activity
lifecycle changes, while allowing a service or native layer to own consent UI
would make permission and teardown behavior difficult to verify.

The architecture requires the sender to own MediaProjection, AudioRecord,
fixed PCM buffers, encoding, packet protection, socket I/O, and reconnect
behavior outside its UI. The repository now contains a capture-only Kotlin/
Compose bootstrap that is build-validated with the checked-in Gradle wrapper and
local SDK; no physical-device capture run or native Opus build has been
validated.

## Decision

The future Android application uses Kotlin and Jetpack Compose for its activity
UI. The activity owns only user interaction and the platform consent launch:

- explain capture limits before requesting consent and present low-rate service
  state after the request; microphone permission, notification permission, and
  projection consent are separate pending states. Repeated start requests while
  authorization is pending or capture is active are ignored, and each async
  result only advances its matching pending state, so stale callbacks cannot
  open overlapping consent flows or create a second capture session;
- transfer the one current consent result directly to the foreground service for
  immediate setup; never persist, cache, or reuse the projection token. If a
  fresh result reaches the same service instance after its one-way teardown has
  begun, reject it and replace the matching consent-wait state with an
  actionable retry failure rather than silently consuming the result;
- emit no PCM, encoded frames, keys, or high-rate metering events to Compose;
  evidence summaries are capped at two state publications per second.

The foreground service is the sole lifecycle owner once a capture request has
been accepted. It owns:

- MediaProjection creation and a registered `MediaProjection.Callback`;
- AudioRecord construction, reads, fixed PCM buffers, and observed capture
  format, reported audio-source/channel-mask values, and actual/minimum buffer
  diagnostics. Those reported integers are local measurement metadata only and
  must not be used to infer why a source is silent or policy-limited;
- bounded non-blocking capture reads so teardown never depends on an unbounded
  platform `READ_BLOCKING` wake-up;
- the bounded encoder, packet-protection, socket, and reconnect workers;
- the visible foreground notification and its explicit stop action;
- service state publication to the activity through a bounded, low-rate state
  model.

`MediaProjection.Callback.onStop()` and the notification stop action use the
same idempotent teardown path. That path stops capture admission, releases
AudioRecord and projection resources, retires PCM/encoded buffers, destroys
session credentials, closes transport resources, and publishes a terminal
state. Reader launch, teardown admission, terminal-outcome readiness, reader
exit, and final completion use one atomic bit-state: a reader cannot launch after
teardown begins, a failed `Thread.start()` rolls back the running-reader bit, and
no completion claimant can win before the selected terminal outcome and the
reader stop/interrupt requests are published. Exactly one claimant then waits for
a successfully launched reader to exit before releasing resources. Duplicate
stop/error notifications may retry the completion check but cannot retire the
same capture resources twice. The service start-admission policy distinguishes
active duplicate starts from a start delivered to a retiring generation: active
duplicates remain no-ops, while a retiring generation cannot admit new consent
or capture resources. Once cleanup is claimed, service references are
retired before platform callbacks run and each release, terminal publication,
foreground-removal, and service-stop step is attempted independently. One
ordinary platform cleanup exception cannot prevent the remaining retirement
steps. A fatal JVM failure is retained while the bounded steps finish and is then
rethrown rather than silently converted into successful teardown. When only
ordinary exceptions occur, the service emits one fixed diagnostic containing
only the bounded failed-step count; it never logs exception text or capture data.
Process recreation, a revoked
projection, or a stale control session fails closed and requires a fresh consent
result and fresh session material.

Any future JNI/NDK component is limited to the codec and bounded buffer boundary
defined by ADR 0009. Kotlin remains responsible for Android permission,
MediaProjection, AudioRecord, foreground-service, notification, and network
lifecycle. The native layer cannot call UI APIs, retain a projection token, or
own an unbounded queue. Its caller supplies fixed direct buffers and receives
only bounded result/error codes.

The first Android implementation is capture-only: it records actual platform
behavior and may report “no capturable audio detected” with possible causes. It
does not claim source-specific capture denial, pair devices, send a production
audio stream, or make a latency promise. The no-audio transition requires two
seconds worth of completed silent PCM frames at the observed sample rate. It is
not based on read-call count because non-blocking `AudioRecord` reads may be
partial; any completed non-silent read resets the silent-frame interval. Every
non-teardown `AudioRecord.read` failure increments the bounded `readErrors`
evidence counter whether Android reports a negative error code or throws a
read-call exception. Every positive result is rejected before sample access unless
it is within the fixed request and exactly channel-aligned; a contract violation
counts as a read error and ends that capture generation. Pre-read permission
checks and teardown wake-ups are not read errors. Capture start, read
completion, and snapshot timestamps reject negative values as invalid local
monotonic-clock readings. A nonnegative snapshot that precedes the recorded
start is still clamped to zero duration so an ordinary clock regression cannot
publish a negative interval. The low-rate evidence publication throttle applies
the same negative-time rejection; after a nonnegative regression it permits one
snapshot and rebases its cadence instead of suppressing updates until the old
timestamp is reached again.

The production Android source set remains capture-only until a later ADR admits
the next boundary. CI checks the main manifest, production dependency
declarations, and production Kotlin imports for direct networking, TLS/crypto,
Android Keystore, Opus, and NDK integration. ADR 0008's Keystore and loopback
TLS spikes live in the separate `:tls-provider-spike` Android test app so its
test-only `INTERNET` permission and Conscrypt dependency cannot execute in the
capture app's UID. The guard is a direct-boundary check, not proof that the
capture implementation cannot reach every indirect dependency or that an
instrumentation spike proves a production capability.

`tools/android_capture_preflight.py` is a separate, read-only lab helper. It
requires an explicitly selected ADB serial but does not print that serial. It
collects the connected device's manufacturer/model, Android release/API/build,
ABI, and installed capture-package state before an operator begins a matrix
row. It does not install an APK, change a permission, request projection,
inspect PCM, or automate any capture scenario, so its output is preparation
metadata rather than capture, policy, lifecycle, power, or latency evidence.

`tools/check_android_capture_record.py` validates an operator-authored,
machine-readable Phase 0A record after a run. Its closed schema requires the
device/build, observed capture format and buffers, initial/final local evidence
snapshots, terminal state, lifecycle presence observations, and device-condition
fields appropriate to the run. It mirrors the capture format's bounded 20 ms
read calculation and validates counter/offset monotonicity. Schema version 2
also records completed silent frames separately from silent read calls and
checks that silent frames never exceed total captured frames; version 1 remains
accepted for older operator records. The validator does not derive a sample
cadence, capture policy, source identity, or latency result.
Any recorded terminal teardown state requires absent notification, service, and
projection observations, matching the service's one teardown path.
The schema has no fields for PCM, source application names, ADB serials,
projection tokens, keys, or opaque device identifiers. The tool is an offline
record-integrity check; it does not connect to ADB, install or launch the app,
grant/revoke permissions, or automate the physical matrix.

## Security, privacy, and realtime implications

The consent result, captured PCM, encoded audio, pairing secrets, session keys,
and raw device identifiers are never logged, serialized into UI state, or placed
in diagnostics exports. Display names and observed source behavior remain
untrusted presentation data. Capture, encode, and transport workers must have
fixed capacity and explicit overflow behavior; the UI only observes summaries.

Projection revocation is an authorization boundary, not a recoverable pause.
The service must not continue using a pre-revocation AudioRecord, media key, or
socket state after teardown begins. Silence after a valid consent result is an
observable symptom, not evidence about a particular source application.

## Required evidence before capture implementation is accepted

Host JVM tests exercise reader-start rollback, rejection of a reader after
teardown admission, duplicate-reader rejection, concurrent launch/teardown
ordering, outcome-publication ordering, exactly-once finish claims, fresh-start
rejection during teardown, read-call exception accounting, continuation after
an ordinary cleanup exception, and deferred fatal-error propagation. These tests
validate only the pure
coordination helpers; they do not establish Android service, AudioRecord, or
MediaProjection lifecycle behavior on a device.

- capture-only runs on the reference Android 12-17 matrix, recording actual
  AudioRecord format, channels, buffer sizes, read errors, and silent periods;
- lock, backgrounding, notification stop, process recreation, and
  `onStop()` each exercise the same resource-retirement invariants;
- Android 14+ foreground-service and one-consent-session behavior is observed
  on real devices;
- power, CPU, memory, thermal state, and foreground-notification behavior are
  recorded during a locked-screen run;
- allowed, idle, and silent/protected-source cases are reported with the
  product's non-definitive wording;
- any JNI codec prototype proves direct-buffer bounds, ABI packaging, and no
  ownership of Android lifecycle APIs before it is connected to capture.

These results are phase 0A evidence only. They do not validate local-network
discovery, TLS pairing, Opus quality, Windows output, or physical latency.

## Rollback plan

Keep `apps/android/` as documentation-only and retain the Rust core/platform
contracts. A failed capture feasibility result removes the Android prototype
without changing protocol, desktop UI, receiver audio, or pairing boundaries.
