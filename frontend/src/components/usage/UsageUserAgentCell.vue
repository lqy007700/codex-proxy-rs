<script setup lang="ts">
import { ZButton, ZPopover } from '@codex-proxy/ui'
import { computed, useTemplateRef } from 'vue'
import { useCopyText } from '@/composables/useCopyText'
import { stablePresetVisualToneClass } from '@/utils/color'
import { summarizeUserAgent } from './shared/userAgent'

const props = defineProps<{ userAgent?: string | null }>()
const summary = computed(() => summarizeUserAgent(props.userAgent))
const copyText = useCopyText()
const panel = useTemplateRef<HTMLElement>('panel')
const visualTone = computed(() => stablePresetVisualToneClass(props.userAgent))
</script>

<template>
  <ZPopover v-if="summary" trigger="click" placement="bottom-end" role="dialog" @opened="panel?.focus({ preventScroll: true })">
    <template #reference="{ open, id }">
      <button
        type="button"
        class="grid max-w-full gap-1 rounded-cp-sm text-left text-cp-sm leading-[1.4] outline-none hover:text-cp-primary focus-visible:ring-2 focus-visible:ring-cp-control-outline"
        :aria-label="`${summary.label} #${summary.code}，查看完整 User-Agent`"
        :aria-expanded="open"
        :aria-controls="id"
        aria-haspopup="dialog"
      >
        <span class="max-w-full truncate font-emphasis" :title="summary.label">{{ summary.label }}</span>
        <span class="w-fit rounded-cp-sm px-1.5 py-0.5 font-mono text-cp-sm leading-none font-bold whitespace-nowrap" :class="visualTone">
          {{ summary.code.slice(0, 4) }} {{ summary.code.slice(4) }}
        </span>
      </button>
    </template>
    <div ref="panel" tabindex="-1" class="grid w-80 max-w-[calc(100vw-2rem)] gap-3 p-3 text-cp-sm outline-none">
      <div class="flex items-center justify-between gap-3">
        <span class="font-heavy">User-Agent</span>
        <ZButton size="small" @click="copyText(userAgent || '', { successText: '已复制 User-Agent' })">
          复制
        </ZButton>
      </div>
      <div class="max-h-60 overflow-y-auto font-mono leading-relaxed wrap-anywhere whitespace-pre-wrap text-cp-text-secondary">
        {{ userAgent }}
      </div>
    </div>
  </ZPopover>
  <span v-else class="text-cp-text-quaternary">—</span>
</template>
