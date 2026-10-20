<script setup lang="ts">
import { ZPopover } from '@codex-proxy/ui'
import { WifiOff } from '@lucide/vue'
</script>

<template>
  <ZPopover trigger="hover-click" placement="top" role="tooltip">
    <template #reference="{ open, id }">
      <button
        type="button"
        class="inline-flex size-6 shrink-0 cursor-help items-center justify-center rounded-full border-0 bg-cp-warning-container text-cp-warning-on-container outline-none transition hover:bg-cp-warning-container-hover focus-visible:ring-2 focus-visible:ring-cp-control-outline"
        aria-label="WebSocket 临时降级"
        :aria-describedby="open ? id : undefined"
      >
        <WifiOff class="size-3.5" />
      </button>
    </template>

    <p class="m-0 max-w-56 px-3 py-2 text-cp-sm text-cp-text-secondary">
      WebSocket 临时降级，可通过恢复状态清除
    </p>
  </ZPopover>
</template>
