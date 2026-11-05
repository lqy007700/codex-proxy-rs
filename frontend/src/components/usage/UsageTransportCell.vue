<script setup lang="ts">
import { computed } from 'vue'
import { usageTransportType } from './shared/presenter'
import UsageTransportBadge from './UsageTransportBadge.vue'

const props = defineProps<{
  upstream?: string | null
  client?: string | null
}>()
const upstreamLabel = computed(() => usageTransportType(props.upstream))
const clientLabel = computed(() => usageTransportType(props.client))
const description = computed(() => `上游 ${upstreamLabel.value} · 接入 ${clientLabel.value}`)
</script>

<template>
  <span class="inline-flex items-center overflow-hidden rounded-full" tabindex="0" :aria-label="description" :title="description">
    <UsageTransportBadge :transport="upstream" joined />
    <UsageTransportBadge v-if="upstreamLabel !== clientLabel" :transport="client" joined />
  </span>
</template>
