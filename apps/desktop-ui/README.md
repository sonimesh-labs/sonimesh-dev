# Desktop UI

Vue/Vite control surface for desktop lifecycle commands. `src/bridge.ts` is the
only frontend integration point: it currently provides a preview adapter and is
intended to be replaced by low-rate Tauri commands after the desktop shell is
introduced. The preview state is explicitly labelled in the UI and must not be
read as an active receiver, a transport measurement, or a hardware capability.
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
seconds the session panel projects while the session is reconnecting, matching
the Rust snapshot's grace countdown without running a real timer. The
session owner's expiry transition is also simulable from the session panel
(`Simulate session expiry`): a trusted session (ready, streaming, reconnecting,
or output unavailable) retires to `idle` while the consumed pairing record stays
retained for audit, so the revoke path remains reachable. The
diagnostics panel can simulate a burst of transport and recovery activity
(`Simulate metrics burst`), so the cumulative-counter formatting and health
cards can be exercised without live audio. A preview
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

```powershell
npm install
npm test
npm run dev
npm run build
```
