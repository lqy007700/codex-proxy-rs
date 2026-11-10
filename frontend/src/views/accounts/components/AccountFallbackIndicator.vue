<script setup lang="ts">
import { ZPopover } from '@codex-proxy/ui'
import { ShieldCheck } from '@lucide/vue'
</script>

<template>
  <ZPopover trigger="hover-click" placement="top" role="tooltip">
    <template #reference="{ open, id }">
      <button
        type="button"
        class="inline-flex size-6 shrink-0 cursor-help items-center justify-center rounded-full border-0 bg-z-warning-container text-z-warning-on-container outline-none transition hover:bg-z-warning-container-hover focus-visible:ring-2 focus-visible:ring-z-control-outline"
        aria-label="保底账号"
        :aria-describedby="open ? id : undefined"
      >
        <ShieldCheck class="size-3.5" aria-hidden="true" />
      </button>
    </template>

    <p class="m-0 max-w-56 px-3 py-2 text-z-sm text-z-text-secondary">
      保底账号，仅在普通账号恢复失败或无可用普通账号时使用
    </p>
  </ZPopover>
</template>
