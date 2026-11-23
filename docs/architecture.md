# SoniMesh Architecture

> Status: architecture draft for feasibility validation
> Version: 1.0.0
> Updated: 2026-11-23

This document describes the intended product boundary and engineering constraints
for SoniMesh. It deliberately separates verified platform facts, implementation
requirements, and capabilities that still require a prototype.

## 1. Executive summary

SoniMesh sends Android playback audio that the Android platform permits the
application to capture to a desktop computer on the same local network. The
desktop receives, authenticates, decrypts, decodes, buffers, resamples, and
plays the stream through the selected system output device.

The desktop control domain may project session phase, redacted invitation
lifecycle, bounded reconnect-grace remaining time, and redacted output
lifecycle through a low-rate `DesktopControlSnapshot`. That projection is
built through the pairing owner's
`snapshot_at(session, now_ms, &output_controller)` call, which observes the
pairing clock and the reconnect-grace clock before projection and reads the
output lifecycle from its actual
owner. The `ControlToMixerBridge` composition object exposes the same
projection through `desktop_snapshot_at(pairing, now_ms)`, observing the
pairing clock and the reconnect-grace clock and reading the output lifecycle
from the bridge's own mixer
session. `DesktopControlOwner` now composes that bridge with the pairing
controller and diagnostic export action for low-rate commands such as snapshot
polling, invitation lifecycle, output inventory/selection, stream-local
gain/mute, and bounded export. Output selection is retained for the next output
lifetime and cannot mutate an active callback stream. A
shell-neutral `DesktopControlView` combines that lifecycle projection with the
current output inventory, retained selection, single-stream gain/mute state, and
redacted active-stream metrics; it leaves multi-stream controls identity-addressed
instead of guessing. A
`DesktopControlOwner` also owns the low-rate control-session and stream-lifecycle
delegation boundary. Its `execute_control_action` entry point dispatches the
coordinator's negotiated/start/rotate/stop actions through one token-checked
completion path, while receiver pipelines remain injected by the application
composition layer. `DesktopReceiverOwner` now composes that owner with one
injected receiver pipeline and exposes an accept-and-execute boundary for the
future shell. After output recreation it can also re-admit a persisted stream
using its owned pipeline, before reporting that stream ready. Its UDP specialization also exposes low-rate poll/decode calls
and the bound media endpoint; it still does not choose a decoder, socket, or
output backend. The receiver scheduler also pumps decoded PCM through the
per-stream format adapters outside the output callback. Each tick stops pumping
an adapter at underflow or when its bounded output queue is full, and reports
conversion errors independently from control and UDP results. A
combined UDP poll/decode tick uses the immediate-drain path: a received packet
does not establish whether further packets are in flight. Callers that enable
target-gated playout must supply that signal explicitly. The combined tick
still requires external frame pacing. A separate monotonic-time scheduler path
polls a bounded packet burst on each call and performs at most one immediate
drain decode per call. It preserves frame-clock phase, skips missed deadlines,
and spaces decode decisions by at least half a frame. After a long scheduler
stall it retires queued audio and drains accumulated UDP in bounded batches
before resuming from fresh packets. It resets its cadence when
the matching stream or output is unavailable. The host must drive it more often
than the frame interval; a live platform host still needs to adopt this path.
The scheduling result distinguishes idle, mismatched media identity, output
failure, pending stream re-admission, and a ready media path. The preconfigured
pipeline is not yet built from Start. Known pipeline contexts are checked
against the requested media identity and direction before admission and a
streaming reply. A pipeline configured with the initial Opus frame policy also
checks the requested sample rate, channels, and frame duration; generic adapters
with unknown context or format remain the host's responsibility.
A successful control reply does not by itself establish media readiness.
A `DesktopControlSession` can now attach a trusted post-pairing channel to that
owner. It polls the existing bounded control framing, executes at most one
envelope per tick, and sends correlated `StreamStatus` replies for start,
rotation, and stop actions. Owner rejections become correlated `ControlError`
replies and do not retire the authenticated transport. A successful Hello gets a
correlated Hello response containing the negotiated version and capabilities.
Completed duplicate start/stop commands receive correlated status replies;
pending duplicates receive a retryable error. Received status/error envelopes are
ignored. The adapter exposes explicit flush/closed states and
does not enter the realtime output callback. A
future Tauri shell can delegate to this owner without moving UI code into the
realtime path. The projection excludes pairing IDs,
proofs, addresses, session material, PCM,
encoded media, realtime callbacks, platform output handles, and device-selection
metadata. Within its redacted output value, `stream_active` and
`fault_reported` describe lifecycle state: an explicit stop clears both, while a
backend health fault retires the stream and sets `fault_reported` until a
successful start or recovery. `health` is the latest low-rate backend health
observation and may remain non-healthy after an explicit stop; a control-surface
consumer must therefore present a fault only when the lifecycle is active or
`fault_reported` is set, rather than treating a stale health value alone as a
current fault.

The system has two planes:

- The control plane performs discovery, pairing, capability negotiation,
  session lifecycle, statistics, and user commands over a reliable authenticated
  connection.
- The data plane carries encrypted Opus packets over UDP. It must remain
  independent from the UI and must use bounded memory.

The separate Android audio lab may exercise the protected media plane on a
controlled local network using a manually entered desktop IPv4 address and an
ADB-reversed connection for its temporary session secret. This is a development
path rather than discovery, pairing, or the production control plane.
The Android lab can select media, game, unknown, or all three playback usages
for a capture session; platform and source capture policies still apply.
The desktop UDP lab can accept optional console gain and mute commands and
publish them to its existing per-stream output control. These controls are
local to the lab and do not implement the desktop UI bridge.
If its output reports a runtime fault, the UDP lab keeps the media session alive
while it retries the selected output outside the realtime callback. A successful
restart uses a fresh PCM endpoint and resumes from new media arrivals.
The paired audio lab can optionally report low-rate packet, decode, and output
recovery state while a session is active. These diagnostics are sampled outside
the output callback and do not prove audible playback or identify why capture
is silent.
An isolated Tauri desktop probe can display Windows output devices and their
default formats and play an explicitly requested short test tone through one
selected endpoint. It is a platform experiment, separate from the synthetic
receiver preview and from a desktop session or system output selection.

The first product goal is not synchronized multi-device playback, recording of
calls, remote desktop, or a universal Bluetooth receiver. It is a practical
local audio bridge for a PC user who wants to listen to permitted Android media
through the same headset or speaker used by the PC.

## 2. Product scope

### 2.1 In scope for the WLAN MVP

- one Android sender and one Windows desktop receiver
- explicit MediaProjection consent before capture
- capture of permitted media and game playback audio
- Opus encoding and encrypted UDP transport
- authenticated pairing and persistent device trust
- mDNS/DNS-SD discovery through a platform-appropriate API, plus manual address
  or QR-code fallback
- one desktop output device at a time
- per-source mute and gain control on the desktop
- bounded jitter buffering, packet-loss concealment, reconnection, and drift
  compensation
- transport, buffering, output, and capture diagnostics

### 2.2 Non-goals

- bypassing Android capture policy, DRM, or protected content restrictions
- phone, cellular, or VoIP call recording
- video transport, remote desktop, or audio/video synchronization
- Internet traversal, cloud relay, or cross-NAT operation
- professional recording, ASIO guarantees, or studio-grade synchronization
- iOS/iPadOS playback capture
- Bluetooth audio as a guaranteed application-readable PCM source
- a full desktop system mixer or a replacement for the operating system mixer

### 2.3 Product truthfulness rule

The Android capture API does not provide a universal reason for silence. The
application can identify authorization loss, transport failure, and output
failure, but it cannot always distinguish “nothing is playing” from “the source
application or content cannot be captured”. The product must display:

> No capturable audio detected. The source may be idle, protected, or refusing
> playback capture.

It must not present that inference as a definitive source-application diagnosis.

## 3. Support tiers and capability boundaries

### 3.1 Active validation baseline

| Platform | Active baseline | Rationale |
| --- | --- | --- |
| Android sender | Android 12–17 | Fewer legacy lifecycle variants while covering current permission behavior |
| Windows receiver | Windows 11 | Primary supported desktop OS in 2026 |
| Windows 10 | Best effort, 22H2/ESU only | Ordinary Home/Pro support ended on 2025-10-14 |
| Linux receiver | Later milestone | PipeWire integration is a separate platform project |
| macOS receiver | Later milestone | Output support is feasible; Bluetooth receiver behavior is not assumed |
| iOS/iPadOS | Not planned | No equivalent product path for system playback capture |

Android 10 and 11 remain API-level compatibility candidates because playback
capture starts at API 29. They are not release-qualified until the device
matrix demonstrates reliable foreground-service, background, and network
behavior.

### 3.2 Android capture facts

Playback capture requires all relevant conditions to hold:

- the source uses a capturable audio usage such as USAGE_MEDIA, USAGE_GAME, or
  USAGE_UNKNOWN;
- the source application and player policy allow capture;
- both applications are in the same Android user profile;
- the user grants the MediaProjection request;
- the authorization remains valid; and
- the device vendor does not add a restrictive implementation detail.

The application must handle the platform returning silence after authorization
loss or when the source is not capturable. MediaProjection.Callback.onStop() is
the authoritative signal for projection-session invalidation.

The capture-only prototype lets the operator select Media, Game, Other, or
All supported usages before each session. The last option requests
`USAGE_MEDIA`, `USAGE_GAME`, and `USAGE_UNKNOWN` together. This selection
filters the playback-capture request for that session; it does not identify the source
application or override the source application's capture policy. The observed
`AudioRecord` format remains the authoritative local measurement.
The start action reads the current selection even if the UI has not yet redrawn
after the operator changes it.

Android 14 and later require the mediaProjection foreground-service type and
single-session consent handling. Android 17 makes local-network protection
mandatory for applications targeting SDK 37 or later. The discovery design must
therefore support both a system-mediated NsdManager picker path and an explicit
ACCESS_LOCAL_NETWORK path where broad local-network access is truly required.
The provisional decision is recorded in
[ADR 0014](./adr/0014-android-17-local-network-discovery.md) and remains
pending Android 17 device evidence.

### 3.3 Bluetooth boundary

Windows AudioPlaybackConnection is a system playback connection API. Its
documented surface creates, opens, closes, and observes a connection; it does
not expose a general application PCM capture stream. Linux Bluetooth audio also
depends on the BlueZ, PipeWire, and WirePlumber stack.

Bluetooth is represented as a capability result, not a promise:

- ApplicationPcmSource — a platform-tested PCM node exists;
- SystemRouted — the system can play the remote device but the application
  cannot read PCM;
- Unsupported — no supported path is available.

The WLAN engine must not pretend that SystemRouted is an input source.

## 4. Quality targets and latency budget

The initial values below are engineering targets for a reference environment,
not market guarantees.

| Metric | Initial target |
| --- | --- |
| Stable-WLAN transport/playout p50 | 80–100 ms depending on mode |
| Stable-WLAN transport/playout p95 | no more than 150 ms |
| Continuous run | 8 hours before MVP; 24 hours before multi-source release |
| Recovery after short network loss | Return to audio within 3 seconds when capture remains valid |
| Memory | No unbounded queue or sustained growth |
| Single-stream loss | 1% random loss without a persistent stall |
| Output | No repeated clicks, pops, or buffer divergence in the reference matrix |

The latency budget must be measured per component:

| Component | Working budget |
| --- | ---: |
| Android capture and frame aggregation | 10–30 ms |
| Encode, encrypt, and send | 1–5 ms |
| WLAN transit | 1–10 ms in the reference network |
| Jitter buffer | 20–60 ms depending on mode |
| Decode, resample, and ring transfer | 2–10 ms |
| Windows output buffering | 10–30 ms, device dependent |
| Lookahead DSP | 0 ms for the single-stream MVP |

If the measured sum cannot meet the target, the implementation must change the
buffer mode or the product target. It must not hide the discrepancy by calling
the result “estimated end-to-end latency”. The UI may show a playout-delay
estimate; true end-to-end latency requires a common-clock hardware or acoustic
measurement.

## 5. System architecture

~~~mermaid
flowchart LR
  subgraph Android["Android sender"]
    AUI["Compose UI"]
    PROJ["MediaProjection consent"]
    AR["AudioRecord"]
    OPUS_A["Opus encoder"]
    SEC_A["Packet protection"]
    CTRL_A["Control client"]
    AUI --> PROJ --> AR --> OPUS_A --> SEC_A
    AUI <--> CTRL_A
  end

  subgraph LAN["Local network"]
    DNS["mDNS / DNS-SD"]
    TCP["Reliable control + TLS"]
    UDP["Encrypted UDP data"]
  end

  subgraph Desktop["Windows receiver"]
    CTRL_D["Control/session service"]
    RX["UDP receive + authentication"]
    JB["Per-stream jitter buffer"]
    DEC["Opus decode + PLC"]
    ASRC["Nominal conversion + drift ASRC"]
    RING["Fixed PCM ring slots"]
    MIX["Realtime mixer"]
    OUT["WASAPI/CPAL output"]
    UI["Tauri UI"]
    RX --> JB --> DEC --> ASRC --> RING --> MIX --> OUT
    UI <--> CTRL_D
    CTRL_D <--> RX
  end

  CTRL_A <--> TCP <--> CTRL_D
  SEC_A --> UDP --> RX
  CTRL_A -. discovery .-> DNS -. service records .-> CTRL_D
~~~

The UI is a control consumer. It never receives PCM or Opus frames and is not
allowed to become part of the realtime scheduling path.

## 6. Execution domains and ownership

The desktop receiver has explicit execution domains:

1. **Control domain** — discovery, pairing, TLS, state transitions, UI commands,
   timers, and diagnostic snapshots.
2. **Network domain** — UDP receive, cheap length/header checks, authenticated
   session lookup, replay filtering, and packet dispatch.
3. **Per-stream domain** — jitter scheduling, Opus decoding, PLC/FEC decisions,
   nominal sample-rate conversion, and drift correction.
4. **Realtime output domain** — fixed-slot reads, gain smoothing, mixing, and
   output-format conversion.

The output callback must not allocate, lock, await, decode, perform network I/O,
write logs, enumerate devices, or call Tauri IPC. It may only access preallocated
state and bounded data structures.

Dynamic stream membership must use fixed slots or a preallocated snapshot/RCU
boundary. Adding a source must not require allocating or taking a long-lived
mutex in the output callback.

The realtime stream membership decision is recorded in
[ADR 0015](./adr/0015-realtime-stream-membership.md) and remains provisional
until Phase 2 mixer evidence exists.

## 7. Android sender

### 7.1 UI and service boundary

The sender should use Kotlin and Jetpack Compose. The foreground service owns:

- MediaProjection lifecycle;
- AudioRecord creation and reads;
- fixed PCM buffers;
- Opus encoding;
- packet protection and UDP send;
- reconnect and network-interface changes;
- a visible notification with an explicit stop action.

Flutter is not prohibited, but if it is retained it must remain a UI shell. No
PCM, Opus frame, socket loop, or per-frame status event may cross a platform
channel. Low-rate state events should be coalesced to approximately 2–5 updates
per second.

### 7.2 Capture path

~~~text
MediaProjection consent
  -> capture configuration
  -> AudioRecord
  -> fixed direct PCM buffer
  -> Opus encoder
  -> packet header + AEAD
  -> UDP socket
~~~

The implementation must record actual AudioRecord format, channel count, buffer
size, read errors, and capture timestamps. It must not silently assume that
every device supplies stereo float PCM at 48 kHz.

Initial parameters to validate:

| Parameter | Candidate | Rule |
| --- | --- | --- |
| Sample rate | 48 kHz | Prefer Opus-native rate; negotiate when required |
| Frame duration | 10 ms and 20 ms | Compare latency, packet rate, CPU, and quality |
| Channels | Mono or stereo | Preserve source layout; do not force expansion |
| Opus application | Audio | Media quality is the default goal |
| Bitrate | 96–128 kbps stereo | Validate by content class and packet loss |
| UDP payload | Usually far below 1200 bytes | Keep a hard maximum to avoid fragmentation |

For the media MVP, DTX should be disabled unless a measured power benefit is
worth the silence-transition behavior. The sender must not request audio focus
merely to capture playback; doing so could change the source application's
behavior.

### 7.3 Lifecycle states

Capture permission, control trust, network reachability, and media streaming are
separate state machines. At minimum the service must handle:

- user denial or cancellation of projection consent;
- projection onStop() and resource release;
- AudioRecord initialization or read failure;
- Wi-Fi handover, address changes, and AP loss;
- process death and service recreation;
- notification stop action;
- control-channel loss while capture remains valid;
- stale session credentials after a reconnect.

The service must never reuse an expired projection token or a stale media key.

## 8. Desktop receiver

### 8.1 Windows output strategy

CPAL is a useful cross-platform abstraction, but its default buffer behavior is
host/device dependent. The Windows MVP must validate the CPAL path against a
small direct-WASAPI adapter before promising a latency target.

The platform layer must support:

- default-device and pinned-device modes;
- device removal and default-role changes;
- unsupported sample formats and channel layouts;
- output stream recreation outside the realtime callback;
- sleep/wake and user-session changes;
- shared-mode operation first, with exclusive mode as a separate experiment.

The output device may require a different nominal rate than 48 kHz. That is a
normal negotiated platform condition, not a protocol error.

### 8.2 Bounded queues

Every queue needs a capacity, a drop policy, and a metric. The policy must be
defined before implementation:

- UDP dispatch full: drop packets already past their playout deadline first;
- jitter buffer full: controlled resynchronization, never unbounded growth;
- PCM ring full: drop a bounded oldest region and fade back in;
- PCM ring empty: output silence/fade; never run Opus PLC in the callback;
- stream reset: discard only the affected stream's buffers.

The initial implementation should publish fixed capacities in a configuration
document and expose the resulting high-water marks in diagnostics. A queue
occupancy snapshot must publish its current level and high-water mark as one
coherent pair, so the reported high-water mark is never below the current level.

## 9. Discovery and control plane

The desktop may advertise _sonimesh._tcp.local, but discovery is not identity.
TXT records should contain only non-sensitive, non-authoritative information:

- protocol version;
- control-service capabilities;
- a temporary instance identifier;
- a user-editable display name;
- a control port, with media ports negotiated after authentication.

A stable public-key fingerprint should not be used as the long-lived discovery
identifier. It enables local tracking and can still be spoofed before trust is
established.

The control channel should use length-prefixed Protobuf envelopes with:

- protocol version;
- request ID;
- message type or oneof payload;
- bounded message size;
- structured error and retryability fields;
- application-level required and optional capabilities.

Commands must be idempotent or define duplicate behavior. Repeating StopStream
must be safe. Each authenticated control transport also receives an opaque,
non-reusable local session token. The control owner rejects a token from a lost
or retired transport before validating its envelope, and a replacement transport
must install a fresh token and repeat hello negotiation. An authenticated
reconnection supplies the fresh media session through a bounded
`MediaSessionAllocator` ([ADR 0017](./adr/0017-control-reconnect-fresh-media-session.md)),
which owns the one-time pairing media root and never re-issues a key context
within a pairing lifetime; the authenticated transport that selects the media
identity remains a separate gate. The pairing/control owner must use the
allocator's terminal revocation composition to drop the media root after trust
revocation, and must observe reconnect grace through the allocator composition
so expiry or a clock regression drops the root with the retired control session;
a failed pairing transition must not clear the root. Both start and stop commands
fail closed until that negotiation succeeds. Prepared stream actions
carry a separate process-unique operation token so a completion routed from
another coordinator cannot match identical pending work. A retry is a no-op only
when the command kind and complete validated payload match; the receiver rejects
identity reuse with conflicting audio parameters and never aliases a start with a
pending stop. SetSourceGain does not belong on the wire in the single-source MVP
when gain is applied locally on the desktop.

## 10. Audio data plane

The initial packet header is a proposal, not a final wire contract:

| Field | Width | Notes |
| --- | ---: | --- |
| Magic | 32 bit | Fast packet-family rejection |
| Version | 8 bit | Data-protocol version |
| Flags | 8 bit | Negotiated frame/content flags |
| Header length | 16 bit | Bounded extension support |
| Session ID | 64 bit | Random media-session identifier |
| Stream ID | 32 bit | Per-session stream identifier |
| Sequence | 32 bit | Loss, reorder, and duplicate detection |
| Sample timestamp | 64 bit | Sender sample position |
| Payload length | 16 bit | Ciphertext payload length |
| Key epoch | 16 bit | Explicit key-rotation generation |

The authenticated data must include the complete header. Header parsing must
validate a minimum size, maximum size, and payload bounds before any allocation.
Only an authenticated packet may reach the Opus decoder.

Sequence, stream, direction, and key-epoch uniqueness must be reflected in the
key/nonce design. Replay-window state must advance only after successful AEAD
authentication. Cheap duplicate filtering may happen earlier, but it must not
mark unauthenticated input as received.

The receiver pins a UDP source endpoint only after the first packet completes
authentication, replay admission, and a bounded jitter decision. Later datagrams
from another endpoint are rejected before parsing and cannot advance replay
state. Key-epoch rotation preserves the pin; changing the pinned source
endpoint requires a fresh authenticated media session and fresh key material
rather than automatic address migration, supplied by the control domain through
the `MediaSessionAllocator` boundary
([ADR 0017](./adr/0017-control-reconnect-fresh-media-session.md)). The sender
may retarget its routing destination freely without rotating media keys: the
destination is routing metadata only and never authenticates the peer.

The UDP media transport is bounded in both directions. The sender transmits
through a non-blocking socket and caps every datagram at
`MAX_MEDIA_DATAGRAM_LEN` (1200 bytes) to avoid IP fragmentation. An oversized
payload is rejected before touching the socket, and the sender never blocks:
a full OS send buffer is reported as backpressure so the caller applies its
own drop or pacing policy. The classification is unit-tested, but a
non-blocking UDP send under sustained overload can also be dropped silently
by the operating system instead of reporting `WouldBlock`; the OS-level
property is environment-dependent and must be validated on the hardware lab
matrix, not assumed from the classification tests. The destination address is
routing metadata only and does not authenticate the peer.

## 11. Jitter, loss, and clock drift

Each stream owns a bounded jitter buffer and a monotonic playout schedule.
The paired audio lab's 20 ms scheduler skips missed frame deadlines and keeps
at least 10 ms between decode decisions; if the next phase-aligned deadline is
closer, the lab shifts it to that minimum interval. This limits catch-up bursts
after host scheduling delays without claiming a physical latency improvement.
Initial user-selectable modes are:

| Mode | Target buffer | Intended use |
| --- | ---: | --- |
| Low latency | 20–40 ms | Stable WLAN, interactive use |
| Balanced | 40–70 ms | Default |
| Stable | 80–120 ms | High jitter or congested WLAN |

The adaptive controller must change slowly, expose minimum/target/maximum water
levels, and reset its integral state after a stream reset. Packet arrival jitter
and clock drift must not be treated as the same signal.

The bounded adaptive target controller
([ADR 0016](./adr/0016-adaptive-jitter-target.md)) now exists in
`sonimesh-audio`: it converts the mode range to frame targets, moves the
target by at most one frame per observation with debounce and hysteresis on
a slow packet-arrival-jitter estimate, requires buffered surplus before
lowering the target, and resets to the mode base on a stream reset. The
deterministic network simulator feeds the controller from virtual-link
arrival jitter for one stream and for two to four streams sharing one
link, with bounded per-stream target trajectories as deterministic
evidence. A deterministic target-gated scheduler simulation now makes
the adaptive target the playout decision: the scheduler waits while fewer
than the target encoded frames are buffered, drains without gating once no
frames are in flight, and forces a playout when the gate deadline expires,
so a mid-stream stall stays bounded and the run always completes. The receiver
pipeline now exposes this decision through an explicit opt-in scheduling
boundary; production output-callback adoption remains future Phase 2 work,
and hardware timing still requires platform evidence.

For the receiver, a bounded relative arrival-jitter estimator in
`sonimesh-audio` converts inter-arrival times into sender-sample units and
reports the per-packet deviation in milliseconds, so the adaptive controller
can be fed from a real packet stream without a shared clock. The receiver
pipeline (`ReceiverStreamPipeline` and the UDP receiver) can now run that
estimator and the adaptive target controller against the live authenticated
packet stream: an opt-in configuration supplies a monotonic arrival clock,
each admitted packet feeds the per-packet deviation and jitter occupancy
into the controller, and the active target state is published through the
per-stream diagnostics. The bounded target-gated playout decision lives in
`sonimesh-audio` as `TargetGatedPlayout` (wait while below the target with
arrivals pending, drain when no frames are in flight, force a playout at
the gate deadline), and both deterministic scheduler scenarios drive their
ticks through it. `ReceiverStreamPipeline` and its UDP wrapper expose an
opt-in `enable_target_gated_playout` plus
`decode_next_with_arrivals_pending` boundary; the caller owns the pending
arrival signal and the ordinary decode method remains an immediate drain path.
Production output-callback adoption and hardware timing remain Phase 2 work.

Recovery policy:

- single missing frame: Opus PLC where the decoder mode supports it;
- in-band FEC: use only when the encoder actually produced usable redundancy and
  the added waiting delay is acceptable;
- repeated loss: PLC followed by a controlled fade, never replay stale PCM;
- late packet: discard after its playout deadline;
- large sequence discontinuity: reset the affected stream rather than allocating
  a large catch-up buffer.

Opus LBRR/in-band FEC belongs to the SILK layer and primarily targets active
speech. The proposed high-bitrate stereo media configuration may operate mainly
in CELT mode, so FEC benefit must be demonstrated rather than assumed. A
deterministic content-class record (see the [in-band FEC content-class
record](./measurements/2026-08-16-windows-opus-inband-fec.md)) demonstrates a
+6.0 dB speech-like gain but no benefit for tone or music-like content; FEC
stays disabled pending a reviewed configuration and loss/CPU/power tradeoff.

Android and the output device have independent clocks. Drift compensation must
use a slow occupancy/error controller, bounded ratio changes, anti-windup, and a
hard resynchronization policy when occupancy leaves the safe region.

A bounded windowed-sinc (polyphase) nominal-rate adapter now covers the
feasibility path for rate mismatches such as 48 kHz to 44.1 kHz. It carries
strong software quality evidence (SNR and passband-flatness regressions in
`sonimesh-audio`), but it is not automatically a hardware-qualified general
converter or a drift controller; nominal conversion and small-ratio clock
correction remain separate stages until hardware measurement selects their
filters.

## 12. Mixing and DSP

All decoded streams enter a common Float32 bus with explicit channel mapping.
The current compatibility mapper operates on interleaved channel indices before
the platform callback: it duplicates mono, zero-extends a smaller layout, and
folds a larger layout by bounded averaging. It does not infer physical speaker
positions or perform sample-rate conversion; those claims require platform
evidence and remain behind the same source boundary.

Gain changes are smoothed across blocks. The single-stream MVP should avoid
lookahead limiting; a multi-source release may add:

~~~text
per-stream gain -> fade/mute -> Float32 sum -> headroom -> optional limiter
~~~

The realtime limiter primitive in `sonimesh-audio` provides the bounded-output
stage: it is allocation-free and lock-free, guarantees that no output sample
exceeds its ceiling (protecting downstream fixed-point conversion), attacks
sample-instantly with zero lookahead, and releases smoothly with a fixed
coefficient. The fixed-slot summing mixer primitive
([ADR 0015](./adr/0015-realtime-stream-membership.md)) owns a preallocated slot
table, iterates active slots in fixed order, and fails closed at admission when
the table is full. Each slot owns its per-stream gain and mute state, published
through a lock-free `GainControlHandle` and ramped inside the callback, and can
carry an optional per-slot `StreamCounters` endpoint that the callback records
underflow, refill-limit, and discontinuity events into without locking.
Membership changes publish through a bounded lock-free command queue
(`CommandDrivenMixer` + `MixerMembershipHandle`): the callback drains and
applies them at the start of each render without allocating or locking. The
app-side Phase 2 boundary is wired in `sonimesh-app`: `MixerRenderSource` owns
the command-driven mixer and the bounded limiter as the realtime source, and
`MixerMembershipController` publishes stream admission/removal and carries each
stream's gain/mute handle from the control domain. The mixer path currently
requires decoded streams to arrive in the output format. The per-stream
source-to-output conversion primitive now exists in `sonimesh-app`
(`StreamFormatAdapter`): it runs in the decode/control domain, drains decoded
source-format blocks, applies nominal sample-rate conversion followed by
channel mapping, and pushes bounded output-format blocks into the stream's
slot queue, and `MixerMembershipController::admit_adapted_stream` installs it
per stream at admission: it accepts any valid source format, builds the
adapter before publishing the slot command, and returns the source producer
together with the adapter for the decode/control domain. The desktop session
wiring composes these pieces in `sonimesh-app`: `MixerOutputController` owns
the platform output and one `MixerRenderSource` per output lifetime, exposing
a clone of the live membership handle; `MixerStreamSession` owns that
controller plus the membership boundary and the per-stream adapter pumps,
starts the output, admits streams through `admit_adapted_stream`, installs
each stream's source producer into its caller-owned pipeline, advances the
adapters after every decode step, and after output-device recreation re-admits
every persisted stream against a fresh empty slot table. Persisted records keep
stable identity-table positions while separately tracking their current mixer
slot, so callers may re-admit streams in any order without overwriting another
record or publishing duplicate membership. Per-stream drift control
(`MixerStreamDriftController`) samples only the bounded adapter
source queue, publishes bounded ppm corrections to the stream's resampler,
retires the adapter epoch on an emergency occupancy threshold, and resets
after recovery. Stream lifecycle is composed in `ControlToMixerBridge`: it
owns the control coordinator and the mixer session together, starts the
platform output with the first stream, admits and removes per-stream
pipelines, commits the coordinator's tokenized completions, and rolls back
partial start, rotation, or stop work so membership and lifecycle stay in
sync; a platform fault retires the membership boundary, and recovery recreates
the source and re-admits every persisted stream. The bridge rejects a direct
`OutputRestored` acknowledgement: only its recovery composition may transition
the lifecycle after a fresh source exists, avoiding a streaming control state
with retired mixer records. Reconnect-grace observation through the bridge,
including low-rate desktop snapshot polling, stops the
output and retires all stream records when expiry or a clock regression
terminates the control session, so the projected output cannot remain live
under an idle lifecycle. The bridge is control-domain code and is not part of
the realtime scheduling path. The 24-hour soak and hardware callback timing
remain open.

Any lookahead, attack, release, or safety limiter delay must be included in the
reported playout budget. Disconnect, mute, reconnect, and device switching use
short fades to prevent clicks.

## 13. UX and diagnostics

The Android UI should explain before consent that some applications, calls, and
protected content may not be capturable. The desktop UI should separate:

- discovery and pairing;
- projection authorization;
- capture activity;
- network quality;
- output-device availability;
- “no capturable audio detected” as a non-definitive diagnosis.

Per-stream diagnostics should include:

- authenticated, duplicate, rejected, late, and reordered packets;
- loss and jitter estimates;
- current/target/min/max jitter and PCM ring levels;
- PLC/FEC use and decode errors;
- resampling ratio/ppm;
- underflow, callback refill-limit, overflow, reset, jitter-resync discarded-frame,
  and reconnection counts;
- playout-delay estimate and its component breakdown.

Current jitter occupancy is published per stream as a coherent (current,
high-water) encoded-frame pair in the diagnostics snapshot, matching the
PCM ring level pair in section 8.2. Each snapshot also carries an active
target-state block — target/min/max jitter water levels, smoothed jitter
in milliseconds, and raised/lowered step counts — published by the
adaptive controller ([ADR 0016](./adr/0016-adaptive-jitter-target.md))
once the receiver pipeline feeds it from live authenticated packets
(section 11). The target water levels, smoothed jitter, raised/lowered
step counters, and target-gated scheduler counters share one versioned
telemetry publication, so a snapshot reads one completed target state with a
coherent `min <= target <= max` triple. Fresh session/reset clears the whole
target telemetry state atomically; explicit scheduler disable clears only its
gated-tick and forced-playout counters while retaining the active controller
water levels. The
product output-callback wiring that consumes
the bounded `TargetGatedPlayout` decision primitive remains Phase 2 work;
the primitive itself (decision, counters, and reset) now lives in
`sonimesh-audio` and is covered by unit and property tests.

Realtime code updates counters only. Structured logs are emitted by non-realtime
domains, use stable event codes, and redact keys, audio payloads, full IPs, and
unnecessary device identifiers.

## 14. Failure handling

| Failure | Required behavior |
| --- | --- |
| Projection revoked | Stop capture, close AudioRecord, destroy media credentials, notify desktop |
| Short Wi-Fi loss | Keep a bounded reconnect window, exponential backoff, no unbounded PCM cache |
| Control channel loss | Retire transport token/pending work; enter grace state; destroy keys after expiry |
| UDP loss | PLC/fade; never repeat old audio indefinitely |
| Output device removal | Fade out and recreate on a non-realtime thread |
| One-stream decode error | Reset only that stream; preserve other sources |
| Version mismatch | Return explicit incompatibility; do not guess |
| Sleep/wake | Revalidate network, session, output device, and timing estimates |
| Firewall denial | Show an actionable private-network firewall diagnosis |

## 15. Repository direction

The intended code layout is:

~~~text
sonimesh/
├─ apps/
│  ├─ android/
│  └─ desktop-ui/
├─ crates/
│  ├─ sonimesh-app/
│  ├─ sonimesh-audio/
│  ├─ sonimesh-network/
│  ├─ sonimesh-protocol/
│  ├─ sonimesh-platform/
│  └─ sonimesh-diagnostics/
├─ proto/
├─ docs/
│  ├─ adr/
│  ├─ architecture.md
│  ├─ roadmap.md
│  ├─ security-model.md
│  └─ testing-and-measurement.md
└─ tools/
   ├─ network-simulator/
   └─ latency-measurement/
~~~

Shared audio, network, protocol, and diagnostic crates must have no Tauri or
web-view dependency. Platform APIs belong behind explicit adapters instead of
being scattered through the audio engine.

## 16. Supply chain and release

Before public distribution, CI and release automation must cover:

- Rust format, Clippy, unit tests, fuzz targets, and platform builds;
- TypeScript type-check, lint, and UI tests;
- Android lint, unit tests, signing, and reproducible build checks;
- generated Protobuf consistency;
- dependency license and vulnerability reports;
- cross-language packet and crypto test vectors;
- signed installers, checksums, and signed update metadata.

Automatic updates must verify a release signature; HTTPS alone is not an update
trust mechanism. Windows packaging must include the firewall and code-signing
plan. Android distribution must document MediaProjection use, privacy behavior,
and background-service expectations.

## 17. Risks and open questions

The highest risks are:

- source applications refusing capture or returning indistinguishable silence;
- Windows output-device and shared-buffer variance;
- Android OEM background restrictions;
- custom UDP cryptography and first-pairing mistakes;
- clock drift and long-running buffer instability;
- the temptation to treat Bluetooth system playback as application PCM input.

Open questions must be answered by evidence, not assumptions:

- Is 20 ms a better default than 10 ms for media quality and CPU cost?
- Which Android OEMs preserve capture and foreground-service behavior while locked?
- What jitter-buffer target survives representative home WLANs?
- Does the selected Opus configuration produce usable FEC for music and games?
  Codec-level evidence on 20 ms speech-like content shows +6.0 dB FEC gain
  over PLC, while a tone and a multi-harmonic music-like profile show no
  benefit (FEC equals PLC at 33.1 dB and 2.4 dB); game-content mixes and the
  latency/CPU/power policy decision remain open (see the [in-band FEC
  content-class record](./measurements/2026-08-16-windows-opus-inband-fec.md)).
- Can CPAL meet the Windows output budget, or is a Windows-specific backend required?
- What Android 17 discovery/permission path gives the best user experience?

See [Security Model](./security-model.md), [Testing and Measurement](./testing-and-measurement.md),
[Roadmap](./roadmap.md), and [Architecture Decision Records](./adr/README.md) for
the implementation gates behind this architecture.
