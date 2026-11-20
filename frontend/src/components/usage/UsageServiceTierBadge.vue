<script setup lang="ts">
import { Zap } from '@lucide/vue'
import { computed } from 'vue'

const props = defineProps<{ tier: string | null | undefined }>()
const label = computed(() => {
  const tier = props.tier?.toLowerCase()
  if (tier === 'fast' || tier === 'priority')
    return 'Fast'
  if (tier === 'ultrafast')
    return 'Ultrafast'
  return null
})
</script>

<template>
  <span
    v-if="label"
    class="inline-flex size-4 shrink-0 items-center justify-center rounded-full bg-z-warning-container text-z-warning-on-container"
    :title="`${label} 模式`"
    role="img"
    :aria-label="`${label} 模式`"
  >
    <template v-if="label === 'Ultrafast'">
      <Zap class="size-2" stroke-width="2.2" aria-hidden="true" />
      <Zap class="-ml-0.5 size-2" stroke-width="2.2" aria-hidden="true" />
    </template>
    <Zap v-else class="size-3" stroke-width="2.2" aria-hidden="true" />
  </span>
</template>
