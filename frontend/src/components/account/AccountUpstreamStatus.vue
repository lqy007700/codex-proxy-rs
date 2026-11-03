<script setup lang="ts">
import type { AccountUpstream } from '@/api'
import { ZPopover } from '@codex-proxy/ui'
import { Server } from '@lucide/vue'
import { computed } from 'vue'
import { upstreamStatus } from '@/utils/account-upstream'

const props = defineProps<{ upstream: AccountUpstream }>()
const status = computed(() => upstreamStatus(props.upstream))
</script>

<template>
  <ZPopover v-if="upstream.lastError === 'unsupported'" trigger="hover-click" placement="top" role="tooltip">
    <template #reference="{ open, id }">
      <button
        type="button"
        class="inline-flex size-6 shrink-0 cursor-help items-center justify-center rounded-full border-0 outline-none transition focus-visible:ring-2 focus-visible:ring-cp-control-outline"
        :class="status.warning
          ? 'bg-cp-warning-container text-cp-warning-on-container hover:bg-cp-warning-container-hover'
          : 'bg-cp-fill-tertiary text-cp-text-secondary hover:bg-cp-fill-secondary'"
        :aria-label="status.label"
        :aria-describedby="open ? id : undefined"
      >
        <Server class="size-3.5" aria-hidden="true" />
      </button>
    </template>

    <p class="m-0 max-w-56 px-3 py-2 text-cp-sm text-cp-text-secondary">
      {{ status.title }}
    </p>
  </ZPopover>
  <span
    v-else
    class="shrink-0 text-cp-xs font-medium"
    :class="status.warning ? 'text-cp-warning-text' : 'text-cp-text-tertiary'"
    :title="status.title"
  >
    {{ status.label }}
  </span>
</template>
