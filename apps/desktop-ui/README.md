# Desktop UI

Vue/Vite control surface for desktop lifecycle commands. `src/bridge.ts` is the
only frontend integration point: it currently provides a preview adapter and is
intended to be replaced by low-rate Tauri commands after the desktop shell is
introduced. It must never receive PCM, Opus frames, keys, or realtime callbacks.

```powershell
npm install
npm run dev
npm run build
```
