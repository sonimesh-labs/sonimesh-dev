<script setup lang="ts">
import { Activity, AudioLines, RefreshCw, SlidersHorizontal } from '@lucide/vue'

import DiagnosticsPanel from './components/DiagnosticsPanel.vue'
import OutputPanel from './components/OutputPanel.vue'
import PairingPanel from './components/PairingPanel.vue'
import RepliesPanel from './components/RepliesPanel.vue'
import SessionPanel from './components/SessionPanel.vue'
import { useDesktopState } from './useDesktopState'

const { busy, createPairing, error, expirePairing, refresh, resetPreview, revokePairing, selectOutput, setGain, setJitterMode, setMuted, simulateCommandRejection, simulateConnectionLost, simulateMetricsBurst, simulateOutputLost, simulateOutputRestored, simulatePairingConsumed, simulateReconnected, simulateSessionExpired, snapshot, start, stop, exportDiagnostics } = useDesktopState()
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
          <strong>{{ snapshot === null ? 'Connecting control service' : snapshot.previewAdapter ? 'Preview adapter active' : 'Control plane online' }}</strong>
          <span>{{ snapshot === null ? 'State pending' : snapshot.previewAdapter ? 'Preview adapter' : 'Desktop bridge' }}</span>
        </div>
      </div>
    </aside>

    <div class="content-shell">
      <header class="topbar">
        <div class="title-block">
          <p class="eyebrow">{{ snapshot === null ? 'Desktop control' : snapshot.previewAdapter ? 'Desktop receiver preview' : 'Desktop receiver' }}</p>
          <h1 v-if="snapshot === null">Connecting to desktop,<br /><span>control service.</span></h1>
          <h1 v-else-if="snapshot.previewAdapter">Preview the receiver,<br /><span>without real audio.</span></h1>
          <h1 v-else>Route your phone audio,<br /><span>without the cloud.</span></h1>
          <p class="topbar-copy">{{ snapshot === null ? 'Waiting for authoritative desktop state.' : snapshot.previewAdapter ? 'Preview adapter only — no audio is captured, transported, or played.' : 'A private, bounded listening session on the output device you choose.' }}</p>
        </div>
        <div class="topbar-actions">
          <span v-if="snapshot" class="session-chip" :class="`chip-${snapshot.session}`" role="status" aria-live="polite">
            <span class="chip-dot" aria-hidden="true"></span>{{ snapshot.previewAdapter ? 'Preview ' : '' }}{{ snapshot.session }}
          </span>
          <button class="icon-action refresh" title="Refresh state" aria-label="Refresh state" :disabled="busy" @click="refresh">
            <RefreshCw :size="18" :class="{ spinning: busy }" aria-hidden="true" />
          </button>
          <button v-if="snapshot?.previewAdapter" class="preview-action" :disabled="busy" @click="resetPreview">
            Reset preview
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
            :reconnect-remaining-ms="snapshot.reconnectRemainingMs"
            @start="start"
            @stop="stop"
            @simulate-connection-lost="simulateConnectionLost"
            @simulate-reconnected="simulateReconnected"
            @simulate-session-expired="simulateSessionExpired"
            @simulate-output-lost="simulateOutputLost"
            @simulate-output-restored="simulateOutputRestored"
            @simulate-command-rejection="simulateCommandRejection"
          />
          <div class="secondary-stack">
            <OutputPanel :devices="snapshot.outputDevices" :selected-id="snapshot.selectedOutputId" :output="snapshot.output" :gain="snapshot.gain" :muted="snapshot.muted" :busy="busy" :preview="snapshot.previewAdapter" @select="selectOutput" @gain="setGain" @mute="setMuted" />
            <PairingPanel :pairing="snapshot.pairing" :busy="busy" :preview="snapshot.previewAdapter" @create="createPairing" @expire="expirePairing" @refresh="refresh" @revoke="revokePairing" @simulate-pairing-consumed="simulatePairingConsumed" />
          </div>
        </div>
        <RepliesPanel :replies="snapshot.replies" :preview="snapshot.previewAdapter" />
        <DiagnosticsPanel id="diagnostics" :metrics="snapshot.metrics" :diagnostic-exports="snapshot.diagnosticExports" :preview="snapshot.previewAdapter" :busy="busy" @set-jitter-mode="setJitterMode" @simulate-metrics-burst="simulateMetricsBurst" @export-diagnostics="exportDiagnostics" />
      </template>
      <div v-else class="loading-state">
        <span class="loading-orbit" aria-hidden="true"></span>
        <p>Connecting to desktop control service</p>
      </div>
    </div>
  </main>
</template>
