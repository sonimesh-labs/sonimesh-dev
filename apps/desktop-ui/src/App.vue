<script setup lang="ts">
import { Activity, AudioLines, RefreshCw, SlidersHorizontal } from '@lucide/vue'

import DiagnosticsPanel from './components/DiagnosticsPanel.vue'
import OutputPanel from './components/OutputPanel.vue'
import PairingPanel from './components/PairingPanel.vue'
import SessionPanel from './components/SessionPanel.vue'
import { useDesktopState } from './useDesktopState'

const { busy, createPairing, error, expirePairing, refresh, selectOutput, snapshot, start, stop } = useDesktopState()
</script>

<template>
  <main class="app-shell">
    <aside class="sidebar">
      <div class="brand"><AudioLines :size="23" aria-hidden="true" /><span>SoniMesh</span></div>
      <nav aria-label="Application sections">
        <a class="nav-item nav-active" href="#session"><SlidersHorizontal :size="18" aria-hidden="true" />Receiver</a>
        <a class="nav-item" href="#diagnostics"><Activity :size="18" aria-hidden="true" />Diagnostics</a>
      </nav>
      <div class="sidebar-footer">
        <span class="sidebar-dot" aria-hidden="true"></span>
        <p v-if="snapshot?.previewAdapter">Preview adapter</p>
        <p v-else>Control bridge</p>
      </div>
    </aside>

    <div class="content-shell">
      <header class="topbar">
        <div>
          <p class="eyebrow">Desktop receiver</p>
          <h1>Receiver control</h1>
          <p class="topbar-copy">One stream, one output, low-rate control state.</p>
        </div>
        <div class="topbar-actions">
          <span v-if="snapshot" class="session-chip" :class="`chip-${snapshot.session}`" role="status" aria-live="polite">
            <span class="chip-dot" aria-hidden="true"></span>{{ snapshot.session }}
          </span>
          <button class="icon-action refresh" title="Refresh state" aria-label="Refresh state" :disabled="busy" @click="refresh">
            <RefreshCw :size="18" :class="{ spinning: busy }" aria-hidden="true" />
          </button>
        </div>
      </header>

      <p v-if="error" class="command-error" role="alert">{{ error }}</p>
      <template v-if="snapshot">
        <div id="session" class="control-grid">
          <SessionPanel :state="snapshot.session" :preview="snapshot.previewAdapter" :busy="busy" @start="start" @stop="stop" />
          <div class="secondary-stack">
            <OutputPanel :devices="snapshot.outputDevices" :selected-id="snapshot.selectedOutputId" :busy="busy" @select="selectOutput" />
            <PairingPanel :pairing="snapshot.pairing" :busy="busy" @create="createPairing" @expire="expirePairing" />
          </div>
        </div>
        <DiagnosticsPanel id="diagnostics" :metrics="snapshot.metrics" />
      </template>
      <div v-else class="loading-state">Connecting to desktop control service</div>
    </div>
  </main>
</template>
