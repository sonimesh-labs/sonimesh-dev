# Desktop UI

Vue/Vite control surface for desktop lifecycle commands. `src/bridge.ts` is the
only frontend integration point. `TauriDesktopBridge` now provides the low-rate
IPC adapter for the future shell: it maps receiver commands to injected Tauri
`invoke` calls and normalizes every returned snapshot as a live (`previewAdapter:
false`) state. The application still selects `PreviewDesktopBridge` by default
until the shell owns a real receiver session. The preview state is explicitly
labelled in the UI and must not be read as an active receiver, a transport
measurement, or a hardware capability.
Its initial ready state includes a synthetic consumed pairing record so the
session controls can be exercised without falsely presenting an inactive
invitation as authenticated. The bridge mirrors the Rust pairing owner's
`revoke` transition: the synthetic record can be revoked, which retires the
trust (pairing → `revoked`, session → `idle`) and makes the invitation-creation
flow (create → active countdown → expire) reachable from the running app. The
preview can simulate the sender verifying the invitation (active -> consumed,
pairing -> ready), so after revocation the full trust cycle can be re-exercised
without a page reload. The
preview also mirrors the session owner's platform events: connection loss and
output loss can be simulated from the session panel (trusted → `reconnecting`,
`streaming` → `outputUnavailable`), with reconnect and output restore returning
to `ready` and `streaming`, so recovery UI is exercisable without hardware.
Connection loss also arms a bounded 30-second reconnect window whose remaining
seconds the session panel projects while the session is reconnecting. The
countdown decays on each snapshot from the armed instant and auto-expires the
reconnecting session at the deadline, matching the Rust snapshot entry point
without running a background timer. A preview clock regression expires the
reconnecting session and any active invitation fail-closed, so a later clock
jump cannot revive trusted state. The
session owner's expiry transition is also simulable from the session panel
(`Simulate session expiry`): a trusted session (ready, streaming, reconnecting,
or output unavailable) retires to `idle` while the consumed pairing record stays
retained for audit, so the revoke path remains reachable. The
diagnostics panel can simulate a burst of transport and recovery activity
(`Simulate metrics burst`), so the cumulative-counter formatting and health
cards can be exercised without live audio. Its adaptive jitter target selector
exposes the documented `Low latency`, `Balanced`, and `Stable` presets and
updates the preview target window and playout delay together. This remains a
preview control until the Tauri bridge owns a real receiver session. An
`Export diagnostics` action issues one bounded diagnostic-export receipt
(monotonic sequence and 16 KiB bundle size) into a most-recent-first receipt
log capped at eight, mirroring the Rust ADR 0011 export journal, so the
retention bound and receipt formatting can be exercised without writing
files. `Download preview JSON` writes the allowlisted synthetic metrics and
receipt list to a browser download for local inspection; it is explicitly a
preview artifact, not a live diagnostic bundle. A preview
`Simulate command rejection` action rejects like a real bridge command so the
control error banner and its recovery can be exercised. A
`Reset preview` action restores the initial synthetic session and counters so
the full preview can be replayed without a page reload. It
must never receive PCM, Opus frames, keys, or realtime callbacks.

The future bridge should project the Rust `DesktopControlSnapshot` after the
pairing owner's low-rate `snapshot_at(session, now_ms, &output_controller)` call. That
entry point observes both the invitation clock and the reconnect-grace clock
before projection and reads output state from its lifecycle owner, then returns
only session phase, a redacted invitation lifecycle, the bounded remaining
reconnect time, and redacted output lifecycle.
Metrics remain separate `StreamCounters` snapshots, and device lists stay on
their own low-rate API. Cumulative counters cross this preview contract as
unsigned decimal strings, including distinct PCM underflow and callback
refill-budget exhaustion counts.

The production adapter uses these command names and payloads: `desktop_snapshot`,
`desktop_start_stream`, `desktop_stop_stream`, `desktop_select_output` with
`{ deviceId }`, `desktop_set_gain` with `{ gain }`, `desktop_set_muted` with
`{ muted }`, `desktop_set_jitter_mode` with `{ mode }`,
`desktop_export_diagnostics`, `desktop_create_pairing`, `desktop_expire_pairing`,
and `desktop_revoke_pairing`. Preview simulation commands intentionally reject
through this adapter so the UI cannot mistake test controls for receiver
operations.

```powershell
npm install
npm test
npm run dev
npm run build
```
