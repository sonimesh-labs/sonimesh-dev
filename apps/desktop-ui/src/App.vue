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
      <div class="brand-block">
        <span class="brand-mark"><AudioLines :size="20" aria-hidden="true" /></span>
        <div>
          <strong>SoniMesh</strong>
          <span>Local audio bridge</span>
        </div>
      </div>

      <nav aria-label="Application sections">
        <p class="nav-label">Workspace</p>
        <a class="nav-item nav-active" href="#session">
          <SlidersHorizontal :size="18" aria-hidden="true" />
          <span>Receiver</span>
          <span class="nav-index">01</span>
        </a>
        <a class="nav-item" href="#diagnostics">
          <Activity :size="18" aria-hidden="true" />
          <span>Diagnostics</span>
          <span class="nav-index">02</span>
        </a>
      </nav>

      <div class="sidebar-footer">
        <span class="sidebar-signal" aria-hidden="true"><i></i></span>
        <div>
          <strong>Control plane online</strong>
          <span>{{ snapshot?.previewAdapter ? 'Preview adapter' : 'Desktop bridge' }}</span>
        </div>
      </div>
    </aside>

    <div class="content-shell">
      <header class="topbar">
        <div class="title-block">
          <p class="eyebrow">Desktop receiver</p>
          <h1>Route your phone audio,<br /><span>without the cloud.</span></h1>
          <p class="topbar-copy">A private, bounded listening session on the output device you choose.</p>
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
          <SessionPanel
            :state="snapshot.session"
            :pairing-state="snapshot.pairing.state"
            :preview="snapshot.previewAdapter"
            :busy="busy"
            @start="start"
            @stop="stop"
          />
          <div class="secondary-stack">
            <OutputPanel :devices="snapshot.outputDevices" :selected-id="snapshot.selectedOutputId" :busy="busy" @select="selectOutput" />
            <PairingPanel :pairing="snapshot.pairing" :busy="busy" @create="createPairing" @expire="expirePairing" @refresh="refresh" />
          </div>
        </div>
        <DiagnosticsPanel id="diagnostics" :metrics="snapshot.metrics" />
      </template>
      <div v-else class="loading-state">
        <span class="loading-orbit" aria-hidden="true"></span>
        <p>Connecting to desktop control service</p>
      </div>
    </div>
  </main>
</template>
