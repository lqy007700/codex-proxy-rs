<script setup lang="ts">
import type { UsageReasoningRecord } from './shared/presenter'

import { Bot, SquareDashedBottomCode } from '@lucide/vue'
import { usageReasoningEffort } from './shared/presenter'

defineProps<{
  record: UsageReasoningRecord
}>()
</script>

<template>
  <span
    class="inline-flex items-center gap-1 whitespace-nowrap text-z-sm font-bold text-z-text"
  >
    <span>{{ usageReasoningEffort(record) }}</span>
    <span
      v-if="record.subagentKind"
      class="inline-flex shrink-0 text-z-purple-text"
      title="子代理请求"
      aria-label="子代理请求"
    >
      <Bot class="size-3.5" stroke-width="2.2" />
    </span>
    <span
      v-if="record.subagentKind === 'review'"
      class="inline-flex shrink-0 text-z-purple-text"
      title="Review 子代理请求"
      aria-label="Review 子代理请求"
    >
      <SquareDashedBottomCode class="size-3.25" stroke-width="2.2" />
    </span>
  </span>
</template>
