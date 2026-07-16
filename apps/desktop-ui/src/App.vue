<script setup lang="ts">
import { MonitorSpeaker, RefreshCw, SlidersHorizontal } from '@lucide/vue'

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
      <div class="brand"><MonitorSpeaker :size="23" aria-hidden="true" /><span>SoniMesh</span></div>
      <nav aria-label="Application sections">
        <a class="nav-item nav-active" href="#session"><SlidersHorizontal :size="18" aria-hidden="true" />Receiver</a>
        <a class="nav-item" href="#diagnostics"><RefreshCw :size="18" aria-hidden="true" />Diagnostics</a>
      </nav>
      <p v-if="snapshot?.previewAdapter" class="adapter-note">Preview adapter</p>
    </aside>

    <div class="content-shell">
      <header class="topbar">
        <div>
          <p class="eyebrow">Desktop control</p>
          <h1>Receiver</h1>
        </div>
        <button class="icon-action refresh" title="Refresh state" :disabled="busy" @click="refresh">
          <RefreshCw :size="18" :class="{ spinning: busy }" aria-hidden="true" />
        </button>
      </header>

      <p v-if="error" class="command-error" role="alert">{{ error }}</p>
      <template v-if="snapshot">
        <div id="session" class="control-grid">
          <SessionPanel :state="snapshot.session" :busy="busy" @start="start" @stop="stop" />
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
