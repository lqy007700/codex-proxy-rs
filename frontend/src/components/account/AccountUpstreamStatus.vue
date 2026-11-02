<script setup lang="ts">
import type { AccountUpstream } from '@/api'
import { computed } from 'vue'
import { upstreamStatus } from '@/utils/account-upstream'

const props = defineProps<{ upstream: AccountUpstream }>()
const status = computed(() => upstreamStatus(props.upstream))
</script>

<template>
  <span
    class="shrink-0 text-cp-xs font-medium"
    :class="status.warning ? 'text-cp-warning-text' : 'text-cp-text-tertiary'"
    :title="status.title"
  >
    {{ status.label }}
  </span>
</template>
