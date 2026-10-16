<script setup lang="ts">
import type { UsageListRecord } from '@/api'
import { computed } from 'vue'

const props = defineProps<{ record: UsageListRecord }>()
const outcomes: Record<string, { label: string, tone: string }> = {
  succeeded: { label: '成功', tone: 'bg-cp-success-container text-cp-success-on-container' },
  failed: { label: '失败', tone: 'bg-cp-error-container text-cp-error-on-container' },
  cancelled: { label: '已取消', tone: 'bg-cp-warning-container text-cp-warning-on-container' },
  incomplete: { label: '未完成', tone: 'bg-cp-warning-container text-cp-warning-on-container' },
  running: { label: '进行中', tone: 'bg-cp-primary-container text-cp-primary-on-container' },
}
const outcome = computed(() => outcomes[props.record.logicalOutcome] ?? {
  label: props.record.logicalOutcome,
  tone: 'bg-cp-fill-quaternary text-cp-text-secondary',
})
</script>

<template>
  <div class="min-w-0 py-0.5">
    <div class="flex items-center gap-1.5 whitespace-nowrap">
      <span class="inline-flex h-6 items-center rounded-full px-2.5 text-cp-sm leading-none font-bold" :class="outcome.tone">
        {{ outcome.label }}
      </span>
      <span
        v-if="record.logicalOutcome !== 'succeeded' || record.attemptCount !== 1"
        class="font-mono text-cp-xs tabular-nums text-cp-text-tertiary"
        :title="`总尝试 ${record.attemptCount} 次`"
        :aria-label="`总尝试 ${record.attemptCount} 次`"
      >
        {{ record.attemptCount }} 次
      </span>
    </div>
    <p v-if="record.message" class="mt-1 mb-0 truncate text-cp-xs leading-[1.45] font-emphasis text-cp-text-secondary" :title="record.message">
      {{ record.message }}
    </p>
  </div>
</template>
