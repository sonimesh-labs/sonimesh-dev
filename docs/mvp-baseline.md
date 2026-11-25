# MVP implementation baseline

> Status: foundation implemented; platform feasibility remains open
> Updated: 2026-11-25

This baseline turns the architecture documents into a buildable core without
claiming that roadmap phases 0A–0D or the Windows WLAN MVP are complete.

## Implemented in the foundation

- one Rust workspace with separate app, audio, diagnostics, network, platform,
  and protocol crates;
- allocation-free version-one media header encoding and bounded packet parsing;
- a 1,200-byte complete-media-datagram limit and a 128-byte header limit;
- exact extended-header preservation for future AEAD associated data;
- a fixed 64-packet replay window advanced only through authenticated ingress;
- per-stream fixed-capacity encoded jitter storage with duplicate, late-frame,
  gap/PLC, controlled-resynchronization, and explicit sequence-exhaustion
  results;
- a preallocated PCM block ring that rejects empty blocks, drops the oldest
  block on overflow, and emits initialized silence on underflow;
- a lock-free cross-thread PCM queue with non-cloneable logical endpoints,
  empty-block rejection,
  oldest-block replacement, and exact discontinuity detection;
- a bounded index-based Float32 channel mapper and preallocated realtime source
  adapter for mono/stereo-to-device layout conversion without callback allocation;
- a bounded windowed-sinc (polyphase) nominal sample-rate converter and
  preallocated source adapter for rate mismatch without callback allocation;
  drift control remains separate;
- a control-domain atomic ppm handle for bounded drift adjustments; hard
  resynchronization remains owned by the stream lifecycle controller;
- a bounded queue-backed realtime render source that preserves partial PCM
  blocks, initializes callback underflow to silence, records refill-budget
  exhaustion separately, caps refill attempts, and fades in after queue
  discontinuities;
  A seeded property walk and a byte-driven libFuzzer target drive queue
  pushes, epoch retirements, render lengths, gain, and mute scripts against
  an independent queue-plus-render model, asserting every rendered output
  slice and every published counter stays in sync after each step.
- a lock-free atomic gain and mute handle shared by a stream's realtime render
  source and the desktop control domain; published targets ramp over a fixed
  128-sample window only when the target changes, apply without allocation or
  locking, and stay bounded to attenuation in the single-stream MVP;
- an app-domain receiver PCM endpoint factory that creates a fresh bounded queue
  pair and format-adapted render source together for output recovery; the
  receiver pipeline can install the paired producer without resetting ingress.
  The factory retains the per-stream gain and mute handle, so a user's choice
  survives output-device recreation without a replay command.
  A mixer-session integration test now proves the survival: distinct per-stream
  gain and mute settings render the expected mixed steady-state levels after
  output recovery, and a gain published to the recovered handle scales only
  its own stream.
- a receiver output lifecycle controller that starts a matching platform source
  before swapping its paired producer into the receiver pipeline; failed output
  starts leave the existing producer intact for retry. The controller accepts
  either the direct receiver pipeline or its UDP composition wrapper without
  taking ownership of ingress or socket state. Output-format diagnostics publish
  only after the platform accepts the matching callback source and clear when
  that source stops. The controller exposes the per-stream gain and mute handle,
  so a desktop owner can publish a local volume choice that reaches the active
  source and survives output-device recovery. It also projects the same redacted
  low-rate output snapshot as the platform output controller through one shared
  projection trait, and the pairing invitation controller reads the desktop
  control snapshot directly from either output owner, so a bridge cannot
  fabricate output lifecycle state.
  A seeded property walk and a byte-driven libFuzzer target drive
  adversarial start, monitor, recover, stop, health, and fail-start scripts
  against an independent state model, asserting the platform call counters,
  producer replacements, published output-format diagnostics, and redacted
  snapshot stay in sync after every step.
- allocation-free gain smoothing, saturating atomic stream and output-status
  counters, endpoint redaction, output adapter contracts, and a tested desktop
  session state machine;
- diagnostic-safe Debug boundaries for media payloads, encoded jitter frames,
  PCM queue blocks, UDP endpoints and network I/O error text, control-envelope
  text, output device metadata, and arbitrary protocol-decoder, decoder, or
  output-backend error details; control-session and operation token
  issuance fails closed instead of wrapping at the `u64` boundary;
- control-domain output health monitoring and stream recreation with a fresh
  render source; platform operations never mutate session state directly, which
  remains owned by the control-session coordinator;
- a Windows Bluetooth experiment that lists system playback candidates and can
  open and close a selected system-routed connection. Local enumeration found
  one candidate, but Windows failed to open it in a later test. The separate desktop output
  probe exposes refresh, connect, wait for phone connection, disconnect, and
  system closure status for the same experiment. Playback and an application PCM
  source remain unverified
  (see `tools/windows-bluetooth-prototype/README.md`);
- a shell-neutral `DesktopControlSession` that attaches the existing bounded
  framed transport to `DesktopReceiverOwner`, executes one authenticated
  envelope per poll, and returns a correlated Hello response and lifecycle
  status/error envelopes, including replies for duplicate stream requests;
  the receiver owner can re-admit its pipeline after output recreation;
  an opt-in host callback can prepare a fresh media context after accepting a
  new start and before admission. Software tests cover preparation failure and
  retry, duplicate requests, and protected UDP with native Opus; the host still
  owns allocator lifetime and already-installed context recovery;
  a live Tauri shell and platform output evidence remain
  open;
- a bounded `DesktopReceiverOwner::tick_media` operation that couples one UDP
  poll with one decode/PLC decision and reports both outcomes to the low-rate
  scheduler;
- a shell-neutral `DesktopReceiverScheduler` that owns the receiver and one
  optional framed control channel, preserving channel ownership across ticks and
  retaining partial control frames and replies across nonblocking polls, and
  retiring the channel on closure or transport failure. Its UDP specialization also
  provides one combined control/media tick and pumps decoded PCM into bounded
  mixer queues. A real UDP loopback test observes non-silent output from that
  composition, using a test decoder and output. Control, media and pump results
  are independent; the caller still owns playout timing and packet polling;
  the native Opus prototype additionally exercises scheduled protected UDP,
  decoding, PLC, and an in-memory output together on loopback. This is software
  integration evidence, not a platform host or audible playback result;
  after three concealed frames and an empty jitter buffer, the scheduled
  receiver resets at the next deadline and resumes from a later packet, while
  buffered future packets still receive their intervening PLC decisions;
  an optional capacity-bounded startup wait begins with a queued frame and is
  reapplied after interruption or output recovery. Loopback tests cover its
  deadlines and recovery with a test decoder and native Opus; the live desktop
  host has not adopted it. On output loss, the scheduler retires buffered media
  and drains accumulated UDP in bounded batches after stream re-admission before
  resuming with fresh packets; this is verified in a loopback software test;
  `mark_control_lost` centralizes the follow-up lifecycle transition and arms
  reconnect grace after a closed or failed control poll. `tick_at(now_ms)` then
  services grace and output-health observation in the same low-rate
  control/media loop, while the reconnect attach entry point installs a fresh
  token/channel pair after trusted reconnection;
- deterministic virtual-clock/network profiles for random and burst loss,
  jitter, duplication, reorder delay, byte-counted bandwidth serialization,
  capacity pressure, and Windows/Linux CI;
- a cross-crate protected pipeline covering AEAD parse/authentication, replay
  rejection, jitter/PLC decisions, non-realtime decode scheduling, and realtime
  PCM transfer without claiming an Opus backend or physical audio result.
  The separate `opus-codec-prototype` now also has a local-audio lab command
  that runs a generated tone through native Opus, encrypted UDP loopback, the
  receiver pipeline, and its render source. An explicit Windows playback mode
  opens the default CPAL output. Its paced relay exercise supports a configurable
  one-second to one-hour software run and fails if the render is silent; it is not
  Android capture or WLAN evidence.
  A separate Android MediaCodec spike generated 100 synthetic 20 ms Opus
  packets on an API 37 x86_64 emulator and desktop libopus decoded them to
  non-silent PCM. This is codec interoperability evidence, not capture, network,
  physical-device, or playback evidence.
  A separate ADB-reversed lab now also captures emulator playback, encodes it
  with MediaCodec, and decodes it on Windows into a software render check or
  CPAL output. This is a loopback lab, not the product's pairing or UDP path;
  physical-device capture, human listening, and physical latency remain open.
  The lab can additionally send ADR 0005-protected UDP packets through the
  existing desktop receiver and decode pipeline. ADB supplies the temporary lab
  secret, not authenticated pairing. A manually addressed controlled-network
  path is available for testing; physical WLAN behavior remains unverified.
  The lab remembers the last valid destination and can keep its desktop listener
  available for successive independent sessions. Optional console commands
  adjust the receiver's local gain and mute during and across those sessions.
  The desktop output probe also controls this lab receiver's gain and mute,
  showing values confirmed by the listener without changing system volume.
  The UDP lab also retries its selected output after a reported runtime fault;
  simulated recovery is tested, while physical device-removal behavior remains open.
  If initial output startup fails, it retains the prepared receiver for a retry
  of the same stream on the control connection, without issuing its media
  identity a second time. A different start retires the retained receiver.
  The Android lab retries a retryable start rejection up to twice on the same
  connection, with a cancellable 500 ms wait between attempts.
  It reports the retry and distinguishes a desktop start rejection from a
  connection failure; the reported reason does not assume why the desktop is
  unready.
  The paired lab also resumes after an authenticated Opus packet rejected as
  invalid: it resets the stream's decode state and waits for fresh media, and
  reports the decode error count. Other decoder failures still end the session.
  A refused initial paired TCP connection is retried briefly without requesting
  another projection consent; an API 37 emulator test exercised this with a
  delayed loopback gate. If an established paired control connection ends, its
  experimental supervisor keeps the current capture/encoder alive, drops queued
  media from the retired session, and retries a fresh paired media session with
  bounded, cancellable monotonic backoff, checking the remaining time once per
  wait so crossing the deadline cannot request a negative sleep. It stops after
  twelve failed reconnect attempts or when the
  operator stops capture. This uses the public fixed lab invitation and a
  keep-listening desktop lab process; it is not production pairing or a
  production reconnect policy. API 37 loopback verification is recorded in the
  development guide; physical WLAN behavior remains open.
  Its Android service can cancel an in-progress paired connection before stream
  startup; after startup, normal shutdown attempts StopStream and moves forced
  TLS socket close off the service main thread. The API 37 loopback reconnect test
  confirms desktop StopStream, service exit, and capture-admission reset after
  reconnect. Host tests cover socket cancellation and ownership handoff, not
  Android device behavior.
  On normal paired shutdown it also attempts a bounded MediaCodec end-of-stream
  drain before finishing the UDP sender. A partial PCM frame is padded with
  silence. An API 37 x86_64 emulator test observes MediaCodec end-of-stream and
  at least one audio packet using the tail-drain helper; it does not verify the
  content of the final packet, the full service path, or a physical codec.
  Once the control connection is ready, the lab reports that audio capture and
  the encoder are being prepared until capture starts. Reconnection and terminal
  status take precedence over that progress message. Its MediaCodec encoder now
  lives in a reusable experimental Android module;
  the main capture-only application does not yet use it.
  The Android lab also reports sustained silent capture and returns to its
  capturing status when signal resumes; this does not establish desktop delivery.
  Its activity-owned test tone stops when capture finishes or the activity leaves
  the foreground, while configuration recreation retains an active tone. API 37 x86_64
  instrumentation covers both transitions; physical-device behavior is untested.
  The lab can also play one reduced-amplitude 48 kHz MLS probe matching the
  latency tool's seed-7 reference during capture. API 37 paired-loopback testing
  observes the probe locally; external recording and physical latency remain open.
  A separate native Opus software test correlates the probe after 64, 96, and
  128 kbps encoding against a known injected delay and codec lookahead; this does
  not validate the Android encoder or a physical audio path.
  An optional paired-listener status report exposes in-session packet and decode
  counts, decoded peak, and output recovery state for controlled-lab diagnosis.
  On normal stop it also reports a final post-drain snapshot; the desktop probe
  retains that snapshot, or the last available update after failure, until the
  next client attempt. These observations do not establish audible playback.
  The Android lab also reports media packets successfully written to its TCP or
  UDP sender, separately from local sender-queue drops; this does not measure
  network delivery or desktop receipt and resets for the next capture.
  The probe also reports whether the listener is waiting for its first accepted
  packet, has received one recently, or has seen no new accepted packet for two
  seconds. State changes are reported promptly while the other metrics retain
  their five-second cadence. This observation does not
  identify the cause of an interruption or establish that audio was decoded.
  Output recovery transitions are also emitted promptly for the desktop probe;
  the cumulative status line remains the compatibility fallback.
  The probe retains the latest 32 bounded lifecycle events for the current
  listener snapshot. Session evidence copy/share includes those events, and
  remains available with status plus events when a session ends before metrics
  arrive. This is operator evidence only and excludes invitations and audio.
  Its fixed startup wait is configurable from 0 to 120 ms for network experiments;
  the desktop probe exposes this setting before listener startup. An opt-in
  experimental Balanced adaptive target gates live playout with bounded waits;
  those waits can produce silence and target reductions do not accelerate
  playback. Fixed scheduling remains the default. Neither mode measures
  end-to-end latency or establishes a quality improvement.
  A bounded UDP relay can apply seeded loss, delay, jitter, duplication and
  reorder delay to live protected lab media sent to an alternate listener port.
  An API 37 emulator-to-Windows software run exercised loss and reorder delay;
  live OS scheduling and physical WLAN behavior remain separate from the
  deterministic virtual-network tests.
  During live playout the paired lab conceals short gaps even when no later
  packet is buffered, then resets an empty stream after three consecutive PLC
  frames to permit fresh audio after an extended interruption. A real UDP and
  native Opus test covers the software path; physical listening remains open.
  After a control-loop stall of at least 160 ms, the paired lab also drops
  accumulated media in bounded batches and resumes with later packets, avoiding
  persistent software backlog; audio already handed to output cannot be recalled.
  A seeded property walk and a byte-driven libFuzzer target drive in-order
  deliveries, duplicates, tampered ciphertext, retired-epoch packets, and
  rotations against an independent replay oracle and counter model,
  asserting failed AEAD never advances the replay window and only
  authenticated in-window packets reach the decoder.
- a generated, length-prefixed Protobuf control envelope with hard frame and
  field limits for controlled-lab command exchange.
- symmetric bounded control framing proven over one real TCP connection: both
  ends read and write length-prefixed envelopes through the same non-blocking
  framing boundary, so round trips, pipelined wire order, and peer-close
  retirement hold in both directions before any authenticated transport is
  added;
- bounded version-one hello negotiation that selects the exact supported
  protocol version and a canonical fixed intersection of known capabilities;
  receiver start and stop commands require this one-time negotiation, and
  starts reject unnegotiated frame durations before media work; each
  authenticated transport has a non-reusable local token rejected after loss or
  retirement;
  control-session expiry retires the token, selected capabilities, and stream
  lifecycle state together; process-unique start/rotation/stop operation tokens
  reject stale or cross-coordinator asynchronous completions; reconnection with
  fresh session material
  requires a newly installed transport token and hello.
- a clock-driven bounded reconnect-grace window that keeps a lost control
  session in the reconnecting lifecycle only until its armed deadline; the
  control-session coordinator expires the token, negotiation, media identity,
  and pending work together when the window elapses or the clock regresses,
  and reconnection with fresh session material ends the window immediately.
- a per-stream, non-realtime decode-worker boundary that schedules ordinary
  decode and PLC from authenticated jitter decisions, rejects empty or
  oversized encoded frames before decoder work, then writes bounded PCM blocks
  to the existing lock-free output queue; no concrete Opus binding yet.
- an application-domain receiver pipeline that composes protected ingress,
  bounded decode admission, and stream-local epoch reset without owning TLS,
  platform output, or a concrete decoder implementation. It can install a
  control-domain authenticated fresh media session without recreating socket or
  output ownership, retiring the prior replay and playout epoch first.
- atomic receiver-pipeline diagnostics for authenticated/rejected/replayed/late
  packets, accepted reordered packets, decoder-side PLC/errors, bounded PCM
  replacement, jitter resynchronization, successful epoch resets, and
  capacity-clamped active-epoch PCM queue level/high water published as one
  coherent atomic pair;
  output-callback underflow remains owned by the future output adapter.
- a bounded per-stream PCM occupancy controller that emits slew-limited ppm
  decisions or stream-local hard-resynchronization requests; hard resync clears
  only that stream's jitter, decoder state, and queued PCM epoch; no
  quality-qualified drift ASRC is selected or performance-claimed yet.
- an app-domain drift coordinator that samples bounded PCM occupancy, publishes
  normal ppm changes to the source handle, and performs the matching
  stream-local hard resynchronization without touching ingress or output state.
  A real-component clock-skew closed loop now drives the receiver pipeline,
  polyphase adapter, mixer, and drift controller under a sustained virtual
  clock offset: a nominal clock stays at the drift target without correction,
  within-cap fast and slow sources publish corrections with the correct sign
  while occupancy stays bounded and rendering continues, and a skew beyond the
  adjustment cap degrades to bounded hard resynchronizations.
  A two-stream real-component scenario adds stream isolation: one stream's
  repeated hard resynchronizations leave a second stream's occupancy,
  correction, and rendering undisturbed.
- a pairing-invitation control owner that synchronizes bounded record expiry,
  monotonic-clock failure, user expiry, and proof outcomes with the desktop
  session state. It invokes a caller-supplied verifier only after bounded
  per-source admission and charges the attempt first; only a consumed proof can
  establish trust. It does not implement TLS, QR payloads, pairing secrets, or
  persistent identity storage.
- ChaCha20-Poly1305 packet protection with HKDF-SHA256 per-stream keys, a
  reviewed nonce layout, sender sequence exhaustion, authentication before
  replay admission, and matching Rust/Kotlin-JVM public vectors; Android runtime
  provider and pairing/TLS root-secret creation remain unimplemented.
- dependency-free Python verifiers independently recompute the published
  pairing, control-frame, media-header, media-key, and media-AEAD vectors from
  RFC 8439 ChaCha20-Poly1305, HKDF-SHA256, and the documented wire layouts,
  reimplement the sender `MediaSequence` exhaustion guard and the receiver
  64-packet `ReplayWindow`, and cross-check the fixed media-plane and
  control-frame limits against this capacities table, so
  vector drift is caught without a second compiler; sealed media packets are
  resealed from constants, not replayed through the Rust code.
- a pure TLS-exporter pairing-binding boundary with fixed Rust/Kotlin test
  vectors for role-separated proofs and the one-time media root. It accepts only
  an externally supplied 32-byte exporter result after the future pinned TLS
  1.3 handshake, performs no TLS, QR, pairing-record consumption, network I/O,
  certificate validation, or persistent identity storage, and therefore does
  not establish pairing or root-secret provisioning.
- a bounded, allocation-free version-one pairing QR payload codec
  ([ADR 0018](./adr/0018-pairing-qr-payload-encoding.md)) with a published
  Rust/Kotlin/Python vector: a 67-byte fixed prefix plus a bounded printable
  UTF-8 control host (1–255 bytes) and exact desktop DER SPKI (32–512 bytes),
  an 834-byte complete-payload maximum, fail-closed length/version/port/expiry/
  host validation before any copy, a borrowed pairing secret so the scanner
  owns zeroization, redacted `Debug`, and a byte-driven libFuzzer target; the
  platform owner that renders or scans an actual QR image remains
  unimplemented.
- a desktop-side pairing handshake composition in `sonimesh-app`
  (`begin_from_rendered_payload` / `verify_scanned_proof`) that chains the QR
  codec, the ADR 0019 proof-frame codec, the pure TLS-exporter pairing
  binding, and the bounded invitation owner: rendering encodes and re-decodes
  its own payload as a self-check, verification rejects a foreign or
  corrupted scan before any derivation, parses the peer's raw proof frame and
  requires the Android-client role byte plus the scanned pairing ID before
  any HKDF work or attempt charging, verifies the carried proof in constant
  time through the real transcript binding (reflected proofs fail at both
  the frame boundary and the binding), charges exactly one admitted attempt,
  and on success consumes the invitation, moves the session to `Ready`, and
  returns the media root bound to that exporter transcript together with the
  exact server-role reply frame to write back. An integration test carries
  that root through `MediaSealer`/`MediaOpener` and the ADR 0017 allocator
  no-reuse record and verifies the reply frame through the peer transcript.
  The TLS exporter is still supplied by the test; no live provider export,
  camera scan, or pairing transport is claimed.
- a host-JVM Android-role pairing handshake composition model in the
  Android test sources that chains the same QR decoder and TLS-binding mirror:
  it derives the published Android proof from one transcript, verifies the peer
  desktop proof, and releases only the matching media root, while foreign or
  corrupted scans and noncanonical exporters fail before derivation. It is
  not production code and proves neither TLS transport, camera scanning,
  attempt limiting, trust storage, nor device interoperability.
- a non-blocking fixed-buffer UDP media receiver composition that rejects
  oversized datagrams before parser ingress, pins the source only after complete
  protected admission, rejects silent endpoint changes without advancing replay
  state, and permits rebinding only after fresh-session installation.
- a bounded `MediaSessionAllocator` (ADR 0017) that owns the one-time pairing
  media root and issues a fresh root-bound grant containing `(secret, context)`
  for an authenticated control-plane reconnection. The full key context is never
  re-issued within a pairing lifetime, the no-reuse record is fixed at 16 entries and fails closed
  when full, and a fresh pairing root replaces the allocator. A seeded property
  walk and a byte-driven libFuzzer target drive arbitrary identity/direction
  supplies against an independent no-reuse model, asserting the exact outcome
  after every step. The authenticated control-plane transport that selects the
  media identity remains gated.
  A seeded property walk and a byte-driven libFuzzer target drive in-order
  deliveries, duplicates, tampered ciphertext, retired-epoch packets,
  oversized datagrams, rotations, and fresh sessions over real loopback
  sockets against an independent pin, replay, and counter model, asserting
  the source pin binds only to the first successful protected endpoint, a
  changed endpoint never moves the pin, and only a fresh authenticated
  session clears the binding.
- a non-blocking fixed-capacity UDP media sender that rejects oversized
  datagrams before touching the socket, never blocks (a full OS send buffer is
  reported as backpressure for the caller's own drop or pacing policy), and
  treats the destination as routing metadata only.
- a bounded session-level UDP media sender that composes the per-epoch sealer
  and the non-blocking socket for one stream: it seals and sends bounded frames,
  reports a full send buffer as backpressure without blocking, refuses a frame
  that exceeds the plaintext limit before touching the socket, rotates to a
  strictly newer key epoch (fresh sequence at zero, same session/stream/direction),
  installs a fresh session with a new context, and treats the destination as
  routing metadata only. An OS `WouldBlock` result classifies as backpressure
  with the consumed sequence, and the bounded sender-side socket-buffer model
  is pinned by the deterministic harness in
  [testing-and-measurement.md](./testing-and-measurement.md).
- an application-domain UDP receiver pipeline that feeds one socket datagram
  through protected ingress, bounded jitter admission, and non-realtime decode;
  control-session and output lifecycle wiring remain separate.
- a bounded adaptive jitter target fed by per-packet relative arrival jitter:
  the controller moves the target water level within configured limits, an
  opt-in receiver-pipeline configuration supplies a monotonic arrival clock and
  feeds live authenticated packets, and the published water levels (one atomic
  set), smoothed jitter, and step counts reach the diagnostic export. A
  bounded target-gated playout decision primitive (`TargetGatedPlayout` in
  `sonimesh-audio`) now implements the scheduler's per-tick decision and its
  bounded gate/counter state, and both deterministic jitter-scheduler
  scenarios drive every playout tick through it. `ReceiverStreamPipeline` and
  its UDP wrapper expose an opt-in `enable_target_gated_playout` plus
  `decode_next_with_arrivals_pending` boundary; the caller owns the pending
  arrival signal and the ordinary decode method remains an immediate drain path.
  Fresh-session installation resets an enabled scheduler's bounded gate streak
  and exported scheduler counters, while explicit disable clears those counters
  and immediately restores that drain behavior. Production
  output-callback wiring and hardware timing remain Phase 2 work.
- an independent LibFuzzer workspace for bounded control decoding, media packet
  parsing, protected ingress, and the bounded diagnostic export journal. The
  ingress target covers arbitrary bytes, valid authenticated arbitrary
  payloads, replay, output bounds, tampering, and plaintext clearing; the
  journal target drives arbitrary record sizes and policy shapes against an
  independent spec model, proving oversized bundles are rejected before any
  record and eviction keeps the newest exports within both fixed bounds.
  Configured finite CI executions are smoke evidence rather than a sustained
  campaign.
- a bounded incremental TCP control-frame reader/writer that validates the
  existing length-prefixed Protobuf envelope and retires partial read/write
  state after a framing, semantic, or non-transient socket error, plus
  Rust/Kotlin-JVM host tests for all five published Hello, StartStream,
  StopStream, StreamStatus, and ControlError wire vectors and their semantic
  bounds. The transport is explicitly unauthenticated and is not a TLS or
  pairing implementation.
- receiver-generated control-plane replies: the app control domain now builds
  bounded `StreamStatus` and `ControlError` envelopes that echo the answered
  command's request ID, re-validate every generated frame against the protocol
  contract, and map coordinator rejections to explicit wire error codes with
  retryability (`RetryLater` for a pending change or a full stream table,
  `UnsupportedCapability` for an unnegotiated frame duration,
  `IncompatibleVersion` for a hello version mismatch, and `InvalidRequest`
  otherwise). Replies survive the untrusted TCP control framing in both
  directions; no TLS, pairing, or key material is involved.
- a capture-only Kotlin/Compose Android phase 0A bootstrap with a per-session
  playback-usage selector for Media, Game, Other, or all three together
  (`USAGE_MEDIA`, `USAGE_GAME`, and `USAGE_UNKNOWN`), plus direct,
  non-persisted MediaProjection consent transfer, a media-projection foreground
  service, a fixed PCM16 AudioRecord read buffer, low-rate state publication
  that rebases after a nonnegative clock regression, bounded evidence counters
  that saturate at the signed 64-bit maximum, validated local monotonic
  timestamps and read-completion offsets, every non-teardown negative or thrown
  AudioRecord read failure, and fail-closed
  validation of positive read bounds and channel alignment; a
  frame-duration-based no-signal transition stable across partial reads;
  and an atomic idempotent teardown gate that transactionally admits reader
  startup, publishes terminal-outcome readiness, rejects a fresh consent result
  delivered to a retiring service generation with an actionable retry state,
  permits one final cleanup claimant only after reader exit, continues bounded
  retirement after an ordinary cleanup exception, and rethrows a fatal JVM
  failure after the bounded attempts finish, while ordinary failures emit one
  count-only redacted diagnostic; partial Vivo physical-device capture and
  API 37 emulator evidence for silent capture, lock-screen continuity, projection
  revocation, notification stop, and process recreation are recorded in the
  [Android capture matrix](./measurements/android-capture-matrix.md). The
  remaining lifecycle, power, source-policy, and OEM matrix is still open.
  On 2026-11-23, an API 37 x86_64 emulator reopened the capture activity during
  an All supported session and observed that usage selected both during capture
  and after stop. This is UI and service lifecycle evidence, not physical-device
  compatibility evidence.
  Consecutive zero-frame reads now end the capture with a retry message after
  five seconds; successful reads, including silent frames, reset that timer.
  Host tests cover the timer and clock regression. The timeout failure path has
  not been injected into a real Android AudioRecord session.
  An activity-owned test tone can exercise the current session's selected usage;
  it stops when the activity leaves the foreground or capture ends. This only
  exercises the app's own playback and does not establish other apps' policies.
  The capture screen also offers a one-button Media/Game/Other self-check during
  one active session and reports per-usage signal observations; this remains a
  test of SoniMesh's own tones, not other apps' playback-capture behavior. A
  dedicated cancel action and the manual tone control can stop the sequence;
  it waits for a measured quiet interval before each tone and reports an
  inconclusive result if quiet cannot be confirmed. Other audio starting during
  a tone can still contribute to the observed signal.
  API 37 instrumentation verifies that cancellation stops the tone. Copy/share
  capture evidence includes the latest completed usage check result from that
  capture session, and the result is retained through activity recreation.
  The capture-only evidence also includes the run's maximum PCM16 sample level;
  host tests cover its calculation and reset; API 37 x86_64 emulator
  instrumentation now confirms a nonzero peak during built-in tone capture and
  passes tone stop/restart/activity recreation, active-session activity
  reopen/source retention, the supported-usage capture flow, and stopping the
  background capture from its notification action. On the API 37 AVD with a
  swipe keyguard enabled, an instrumentation test also confirms capture and the
  foreground notification remain active while the display is off, then permits
  a normal stop after wake. Earlier
  failures coincided with a System UI not-responding dialog and cleared after the
  emulator settled. This is emulator evidence, not physical-device or
  audible-output evidence. The capture status card displays the normalized peak
  since the previous state update, while local observations retain the run peak;
  neither is an instantaneous meter or a loudness measurement.
- a buildable Vue/Vite receiver-control UI with a single low-rate bridge
  boundary, session controls, output selection, per-stream volume and mute,
  pairing-invitation state, and diagnostics presentation for packet,
  PCM-queue/refill, and resampling state; it uses in-memory preview data by
  default and also contains an injectable Tauri IPC adapter for the desktop shell.
  The Rust app crate now also exposes a Tauri-independent `DesktopControlOwner`
  that composes synchronized snapshots, pairing lifecycle, output inventory and
  stopped-state selection, stream-local gain/mute, redacted active-stream metrics,
  and bounded diagnostic export for a future receiver command layer. Its identity-free
  gain/mute commands resolve the unique active stream and reject zero or ambiguous
  stream state; multi-stream callers retain identity-addressed controls. It can also
  persist an export through the existing bounded `ExportStore`. A generic
  `DesktopReceiverOwner` now keeps this control owner beside one injected receiver
  pipeline and executes authenticated stream actions through the same completion
  path. A standalone Tauri shell now exposes the real Windows CPAL output
  inventory and persisted output selection in the app configuration directory;
  selection does not change the system default. A missing or unreadable setting
  uses the default endpoint. Device inventory is cached between desktop
  snapshots and re-enumerated by the output panel's refresh command or when an
  output is selected; a disappeared selection falls back to the current default
  or first available endpoint while retaining the saved preference. Refreshing
  after the endpoint returns restores it. Returning focus to the desktop shell
  also refreshes the inventory; if a command is running, the refresh waits for
  that command to finish. A short low-level test tone can be
  played through the selected endpoint.
  The shell explicitly
  disables pairing, receiver, stream controls, and live diagnostics because it
  does not yet construct the receiver owner. No live receiver, audio, key, or
  realtime integration is connected to the desktop UI process.
- an isolated Tauri output-probe prototype that lists Windows CPAL devices and
  their default formats and can play a short test tone on a selected endpoint.
  It can also start and stop the separately built paired-audio lab process and
  display its public test invitation, session status, and periodic packet,
  decode, and output-recovery observations. That process can use a selected output;
  the probe also retains a bounded ordered history of listener lifecycle events
  for session diagnosis. The probe does not own a product receiver session or
  change the system default output. The observations are not evidence of audible
  playback or latency. A saved lab output is restored by its device label and
  default format rather than its transient list index; missing or ambiguous
  matches require an explicit new selection.
- a dependency-free `tools/latency-measurement` crate that generates a
  deterministic MLS reference WAVE (PCM 16/24/32 or IEEE float 32/64), reads
  and reports the declared properties of WAVE recordings (PCM 16/24/32, IEEE
  float 32/64, and WAVE_FORMAT_EXTENSIBLE) before measurement, estimates the
  correlation offset with a bounded search window and distinctness check, and
  prints per-run offsets plus p50/p95/p99 summaries; it is a software
  measurement aid, not a physical latency proof.

## Initial capacities and policies

| Boundary | Current hard bound | Policy |
| --- | ---: | --- |
| Media fixed header | 36 bytes | Reject truncated input |
| Media extended header | 128 bytes | Reject larger declared header |
| Complete media datagram | 1,200 bytes | Reject before authentication/allocation |
| Control Protobuf frame | 64 KiB | Reject before Protobuf decoding |
| Control frame prefix | 4 bytes | Reject truncated length |
| Control start stream | 8–192 kHz, 1–8 channels, 10/20 ms | Reject invalid command parameters |
| Control detail | 512 bytes | Reject over-long UTF-8 detail |
| Protected media plaintext | 1,148 bytes | Reserve 16 bytes for Poly1305 tag |
| Encrypted payload with fixed header | 1,164 bytes | Extensions reduce this available payload |
| Replay window | 64 packets | Reject authenticated duplicate or too-old packet |
| Jitter storage | Compile-time capacity per stream | Stream-local reset on capacity collision |
| Encoded frame | Compile-time capacity per stream | Reject empty or oversized frame copy |
| Local PCM ring | Compile-time blocks and samples | Drop oldest block; output silence when empty |
| Cross-thread PCM queue | 1–128 inline blocks | Atomically replace oldest; report sequence gap |
| Realtime render refills | 128 block pops per callback | Stop refilling, emit silence, and count one limit hit |
| Per-stream gain control | 0.0–1.0 linear, attenuation-only | Reject non-finite or out-of-range; ramp over 128 samples |
| Output sample rate | 8–192 kHz | Reject outside platform-neutral bound |
| Output channels | 1–8 | Reject zero or excessive layouts |
| Requested callback | default or 16–8192 frames | Fixed request may be unsupported |
| Maximum callback | 16–8192 frames | Oversized callback emits silence and a fault |
| Pairing QR payload | 834 bytes (67 + 255 + 512) | Reject before decoding; host ≤255, SPKI 32–512 |
| Active source count | One for MVP | Separate ingress and buffers per future source |
| Retained media identities per pairing | 16 entries | Fail closed when full; fresh pairing root required |

Compile-time capacities make accidental unbounded growth impossible in these
containers. Concrete frame count and block size must be selected by the frame,
jitter, and output-backend ADRs using measured 10/20 ms and device results.

## Deliberately not implemented

- complete Android capture hardware/OEM evidence, lifecycle matrix results, and
  power measurements; partial Vivo capture plus API 37 emulator lifecycle
  records are documented in the [Android capture matrix](./measurements/android-capture-matrix.md),
  but they do not qualify physical-device or OEM behavior;
- enabling an Opus encoder/decoder binding in the core; prototype-level
  PLC/FEC/DTX, authenticated malformed-payload, and 10 ms vs 20 ms evidence
  is recorded in the dated opus measurement records, and the binding stays
  behind the ADR 0009 gate until Windows/Android ABI, power, and sustained
  coverage-guided fuzz evidence exists;
- first pairing, TLS, QR bootstrap, identity storage, revocation, or actual
  root-secret provisioning. The Windows TLS spike has since proven only that a
  live pinned Rustls 1.3 exporter can drive the pure desktop handshake
  composition in memory on that host; it does not prove Android interoperability,
  a production provider, QR image transport, trust storage, or pairing. AEAD/HKDF
  media protection and bounded epoch rotation exist only after a caller supplies
  an authenticated control-session secret;
- discovery, control-plane address migration/reconnection, and the live
  concurrent accept/server loop. The UDP source pin can be reset by a fresh
  media session, and the untrusted TCP framing reader still cannot authorize or
  supply one by itself. The ADR 0020 type-state wrapper now unlocks bounded
  control framing only after a verified pairing proof exchange over the
  pinned channel; `DesktopControlSession::from_trusted_channel` connects that
  result to the receiver owner, while loopback integration tests drive the
  framing into hello negotiation, the coordinator transport token, a fresh
  allocator identity, and a completed start. The desktop production provider now
  exists behind [ADR 0021](./adr/0021-production-tls-provider.md):
  `sonimesh-network::desktop_tls` establishes pinned Rustls(`ring`) TLS 1.3
  channels, and an application-domain `PairingListener` exposes a non-blocking
  accept poll that hands one accepted stream to the serialized invitation-gated
  handshake. The exchange uses a fixed 30-second post-accept I/O deadline,
  verifies the peer proof through the real pairing owner over the live
  exporter, writes the reply frame back, and the trusted channel negotiates
  hello through the coordinator in a loopback test. This polling boundary does
  not claim a concurrent accept/server loop. The receiver scheduler drains a
  pending nonblocking control reply before polling another command, so temporary
  socket backpressure pauses intake without discarding the session. A 2026-11-22
  emulator/ADB
  reverse-loopback run now records Conscrypt against this desktop stack, but
  physical WLAN/OEM interoperability remains open and ADR 0021 stays
  Provisional. A separate target-SDK 37 discovery spike now records emulator
  request, picker, denied-permission failure, and same-device service found/lost
  callbacks under ADR 0014, but Android runtime provider wiring, physical
  discovery, and any concurrent-accept server loop remain open.
- Windows output backend selection, physical device/power lifecycle evidence,
  and latency qualification. The independent direct-WASAPI prototype and
  controlled same-host callback comparison exist, but do not settle a backend;
- Tauri integration, signing, packaging, and firewall integration;
- a hardware-qualified resampler/ASRC, acoustic latency, and soak validation; the
  windowed-sinc nominal converter carries software quality evidence but does not
  establish physical timing or acoustic quality claims.

An optional Windows CPAL adapter and a separate direct-WASAPI prototype are now
available for phase 0B measurement. Their same-host comparison records actual
callback/render frames and backend-local software timing under matching silent
smoke configuration in the [Windows output backend comparison
record](./measurements/2026-07-20-windows-output-backend-comparison.md). This
does not close the backend-selection, physical device/power lifecycle, or
physical-latency gates in ADR 0003.

A same-host [backend recheck record](./measurements/2026-07-20-windows-output-backend-recheck.md)
and a later [output recheck](./measurements/2026-08-13-windows-output-recheck.md) repeat the
silent smoke against the current reference host, a pinned Realtek endpoint, and a 44.1 kHz
virtual endpoint; neither closes the ADR 0003 gates.

A two-second silent CPAL smoke on the Windows reference host observed callback
sizes beyond both the requested and device-reported frame sizes. The evidence and
remaining gaps are documented in
[the Windows CPAL smoke record](./measurements/2026-07-17-windows-cpal-smoke.md).
The second host sample used an eight-channel USB endpoint; semantic speaker
position mapping and physical source-to-device routing remain unqualified.

These are not omissions hidden behind mocks. Each is a named roadmap/ADR gate,
and no build should leave a controlled lab network before phase 0D is reviewed.

## Next implementation slice

1. Run the capture-only phase 0A app across the evidence matrix under
   [ADR 0010](./adr/0010-android-capture-service-boundary.md); the current
   project is only the buildable bootstrap.
2. Measure CPAL and prototype direct WASAPI behind `sonimesh-platform`; record
   callback size, negotiated format, device lifecycle, and latency in ADR 0003.
3. Validate the provisional Opus binding/build policy in ADR 0009 on Windows and
   Android; the deterministic 10 ms vs 20 ms comparison is now recorded
   (codec-level quality, wire overhead, PLC, and wall-clock CPU), and the
   remaining evidence is Android ABI packaging, power, and sustained
   coverage-guided fuzzing.
4. Implement the reviewed QR-pinned TLS pairing design only after Android
   Keystore and Windows credential-store spikes; do not enable any non-lab
   network build before that validation.
