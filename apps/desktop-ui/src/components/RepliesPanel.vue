<script setup lang="ts">
import { CircleAlert, CircleCheck, MessageSquareReply } from '@lucide/vue'

import type { ControlReply } from '../types'

defineProps<{
  replies: ControlReply[]
  preview?: boolean
}>()

function stateLabel(state: string) {
  return state
}

function errorMeta(reply: ControlReply) {
  if (reply.kind !== 'error') return ''
  const retry = reply.retryable ? 'retryable' : 'not retryable'
  return `${reply.code ?? 'error'} · ${retry}`
}
</script>

<template>
  <section class="replies panel" aria-labelledby="replies-heading">
    <div class="replies-heading">
      <div>
        <p class="eyebrow">{{ preview ? 'Preview answers' : 'Control answers' }}</p>
        <h2 id="replies-heading">Recent control replies</h2>
        <p>{{ preview ? 'Synthetic receiver-generated replies to lifecycle commands; not wire traffic.' : 'Receiver-generated acknowledgements and structured rejections.' }}</p>
      </div>
      <span class="panel-icon"><MessageSquareReply :size="20" aria-hidden="true" /></span>
    </div>

    <ul v-if="replies.length > 0" class="reply-list">
      <li v-for="reply in replies" :key="reply.requestId" class="reply-row" :class="`reply-${reply.kind}`">
        <span class="reply-kind" aria-hidden="true">
          <CircleCheck v-if="reply.kind === 'streamStatus'" :size="15" />
          <CircleAlert v-else :size="15" />
        </span>
        <div class="reply-body">
          <p class="reply-detail">{{ reply.detail }}</p>
          <p class="reply-meta">
            <template v-if="reply.kind === 'streamStatus'">state {{ stateLabel(reply.state ?? 'unknown') }}</template>
            <template v-else>{{ errorMeta(reply) }}</template>
          </p>
        </div>
        <span class="reply-request">#{{ reply.requestId }}</span>
      </li>
    </ul>
    <p v-else class="reply-empty">No control replies yet.</p>
  </section>
</template>