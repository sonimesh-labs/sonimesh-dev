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
to `ready` and `streaming`, so recovery UI is exercisable without hardware. It
must never receive PCM, Opus frames, keys, or realtime callbacks.

The future bridge should project the Rust `DesktopControlSnapshot` after the
pairing owner's low-rate `snapshot_at(session, now_ms, &output_controller)` call. That
entry point observes the invitation clock before projection and reads output
state from its lifecycle owner, then returns only
session phase, a redacted invitation lifecycle, and redacted output lifecycle.
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
