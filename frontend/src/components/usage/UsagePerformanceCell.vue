<script setup lang="ts">
import type { UsagePerformanceRecord } from './shared/presenter'

import { computed } from 'vue'
import { usagePerformanceDetails } from './shared/presenter'

const props = defineProps<{
  record: UsagePerformanceRecord
}>()

const performance = computed(() => usagePerformanceDetails(props.record))
</script>

<template>
  <div class="grid justify-items-end gap-y-1.5 whitespace-nowrap font-mono text-z-sm leading-none font-heavy tabular-nums">
    <span class="text-z-text">{{ performance.throughputDisplay }}</span>
    <span class="text-z-xs text-z-text-secondary">
      <span title="首字时间" class="text-z-text-quaternary">TTFT</span>
      {{ performance.firstTokenDisplay }}
    </span>
  </div>
</template>
