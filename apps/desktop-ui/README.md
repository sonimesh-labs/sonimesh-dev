# Desktop control UI

This directory is reserved for the Tauri 2 and Vue 3 control surface. Product UI
scaffolding is gated on a validated Windows output spike and a recorded backend
ADR, so the feasibility work cannot accidentally turn a CPAL/WASAPI assumption
into an MVP claim.

The UI will consume low-rate session and diagnostic snapshots from
`sonimesh-app`. It must never own or transport PCM, Opus frames, output callbacks,
network loops, or key material.

See [the MVP baseline](../../docs/mvp-baseline.md) for the bootstrap gate.

