<script setup lang="ts">
import { ZPopover } from '@codex-proxy/ui'

import { Info } from '@lucide/vue'

// 表格单元格的“信息点”悬浮明细：统一触发按钮与面板壳。
defineProps<{
  title?: string
  triggerLabel: string
  tone?: 'primary' | 'warning'
}>()
</script>

<template>
  <ZPopover
    trigger="hover"
    placement="right"
  >
    <template #reference>
      <button
        type="button"
        class="inline-flex size-4 items-center justify-center rounded-full outline-none focus-visible:ring-2 focus-visible:ring-z-control-outline"
        :class="tone === 'warning'
          ? 'bg-z-warning-container text-z-warning-on-container hover:bg-z-warning-container-hover'
          : 'bg-z-primary-container text-z-primary-on-container hover:bg-z-primary-container-hover'"
        :aria-label="triggerLabel"
      >
        <Info class="size-3" />
      </button>
    </template>

    <div class="grid w-60 gap-2 p-3 text-z-sm leading-none">
      <p v-if="title" class="m-0 font-heavy text-z-text">
        {{ title }}
      </p>
      <slot />
    </div>
  </ZPopover>
</template>
