<script setup lang="ts">
import type { UsageListRecord } from '@/api'
import { CheckCheck } from '@lucide/vue'
import { computed } from 'vue'
import { usageErrorCode, usageErrorSummary } from './shared/errors'

const props = defineProps<{ record: UsageListRecord }>()
const summary = computed(() => usageErrorSummary(props.record) || props.record.errorMessage)
const code = computed(() => usageErrorCode(props.record))
</script>

<template>
  <div class="min-w-0 py-0.5">
    <div class="flex flex-wrap items-center gap-x-2 gap-y-1">
      <span class="font-mono text-cp-sm font-bold text-cp-error-text">
        {{ record.logicalOutcome === 'incomplete' ? '响应未完成' : '请求失败' }}
      </span>
      <code v-if="code" class="wrap-anywhere font-mono text-cp-sm font-bold text-cp-text-secondary" translate="no">{{ code }}</code>
      <span v-if="record.recovered" class="inline-flex shrink-0 text-cp-success-text" role="img" aria-label="后续已恢复" title="后续已恢复">
        <CheckCheck class="size-4" :stroke-width="2.25" aria-hidden="true" />
      </span>
    </div>
    <p v-if="summary" class="mt-1 mb-0 truncate text-cp-sm leading-[1.45] font-emphasis text-cp-text-secondary" :title="summary">
      {{ summary }}
    </p>
  </div>
</template>
