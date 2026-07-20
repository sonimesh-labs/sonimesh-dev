# Desktop UI

Vue/Vite control surface for desktop lifecycle commands. `src/bridge.ts` is the
only frontend integration point: it currently provides a preview adapter and is
intended to be replaced by low-rate Tauri commands after the desktop shell is
introduced. The preview state is explicitly labelled in the UI and must not be
read as an active receiver, a transport measurement, or a hardware capability.
It must never receive PCM, Opus frames, keys, or realtime callbacks.

The future bridge should project the Rust `DesktopControlSnapshot` after the
pairing owner's low-rate `snapshot_at(session, now_ms, &output_controller)` call. That
entry point observes the invitation clock before projection and reads output
state from its lifecycle owner, then returns only
session phase, a redacted invitation lifecycle, and redacted output lifecycle.
Metrics remain separate `StreamCounters` snapshots, and device lists stay on
their own low-rate API.

```powershell
npm install
npm run dev
npm run build
```
