<script setup lang="ts">
import type { UsageListRecord } from '@/api'
import { ZPopover, ZScrollbar } from '@codex-proxy/ui'
import { computed } from 'vue'

const props = withDefaults(defineProps<{ record: UsageListRecord, showMessage?: boolean }>(), { showMessage: true })
const hasIncompleteError = computed(() => props.record.logicalOutcome === 'incomplete' && !!props.record.message)
const outcomes: Record<string, { label: string, tone: string }> = {
  succeeded: { label: '成功', tone: 'bg-z-success-container text-z-success-on-container' },
  failed: { label: '失败', tone: 'bg-z-error-container text-z-error-on-container' },
  cancelled: { label: '已取消', tone: 'bg-z-warning-container text-z-warning-on-container' },
  incomplete: { label: '未完成', tone: 'bg-z-warning-container text-z-warning-on-container' },
  running: { label: '进行中', tone: 'bg-z-primary-container text-z-primary-on-container' },
}
const outcome = computed(() => outcomes[props.record.logicalOutcome] ?? {
  label: props.record.logicalOutcome,
  tone: 'bg-z-fill-quaternary text-z-text-secondary',
})
</script>

<template>
  <div class="min-w-0 py-0.5">
    <div class="flex items-center gap-1.5 whitespace-nowrap">
      <ZPopover :disabled="!hasIncompleteError" trigger="hover-click" placement="top-start" role="tooltip">
        <template #reference="{ open, id }">
          <component
            :is="hasIncompleteError ? 'button' : 'span'"
            :type="hasIncompleteError ? 'button' : undefined"
            :aria-label="hasIncompleteError ? '未完成，查看错误信息' : undefined"
            :aria-describedby="hasIncompleteError && open ? id : undefined"
            class="inline-flex h-6 items-center rounded-full border-0 px-2.5 text-z-sm leading-none font-bold outline-none focus-visible:ring-2 focus-visible:ring-z-control-outline"
            :class="[outcome.tone, hasIncompleteError ? 'cursor-help' : undefined]"
          >
            {{ outcome.label }}
          </component>
        </template>

        <div class="w-80 max-w-[calc(100vw-2rem)]">
          <ZScrollbar max-height="240px">
            <p class="m-0 p-3 text-z-sm leading-5 whitespace-pre-wrap wrap-break-word text-z-text-secondary">
              {{ record.message }}
            </p>
          </ZScrollbar>
        </div>
      </ZPopover>
      <span
        v-if="record.logicalOutcome !== 'succeeded' || record.attemptCount !== 1"
        class="font-mono text-z-xs tabular-nums text-z-text-tertiary"
        :title="`总尝试 ${record.attemptCount} 次`"
        :aria-label="`总尝试 ${record.attemptCount} 次`"
      >
        {{ record.attemptCount }} 次
      </span>
    </div>
    <p v-if="showMessage && record.message && !hasIncompleteError" class="mt-1 mb-0 truncate text-z-xs leading-[1.45] font-emphasis text-z-text-secondary" :title="record.message">
      {{ record.message }}
    </p>
  </div>
</template>
