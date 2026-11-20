<script setup lang="ts">
import type { UsageModelRecord } from './shared/presenter'

import { CornerDownRight } from '@lucide/vue'

import { computed } from 'vue'
import { usageModelDisplay } from './shared/presenter'
import UsageServiceTierBadge from './UsageServiceTierBadge.vue'

const props = defineProps<{
  record: UsageModelRecord
  serviceTier?: string | null
}>()

const modelDisplay = computed(() => usageModelDisplay(props.record))
</script>

<template>
  <div class="inline-grid max-w-full gap-1">
    <div class="flex min-w-0 items-center gap-1.5">
      <code
        class="block min-w-0 truncate font-mono text-z-sm leading-none font-heavy text-z-text"
        :title="`请求模型：${modelDisplay.primary}`"
      >
        {{ modelDisplay.primary }}
      </code>
      <UsageServiceTierBadge :tier="serviceTier" />
    </div>
    <div
      v-for="route in modelDisplay.routes"
      :key="route.kind"
      class="flex min-w-0 max-w-full items-center gap-1.25 text-z-text-secondary"
      :title="route.description"
    >
      <CornerDownRight
        class="size-3.25 shrink-0"
        :class="route.kind === 'returned' ? 'text-z-orange-text' : 'text-z-blue-text'"
        stroke-width="2.4"
        aria-hidden="true"
      />
      <code class="block truncate font-mono text-z-xs leading-none font-bold">
        {{ route.model }}
      </code>
    </div>
  </div>
</template>
