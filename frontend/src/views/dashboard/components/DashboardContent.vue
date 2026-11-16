<script setup lang="ts">
import type { dashboardSnapshotView } from '@/components/usage/shared/dashboard'
import RequestHealthTimelineCard from '@/components/usage/RequestHealthTimelineCard.vue'

type DashboardSnapshotView = ReturnType<typeof dashboardSnapshotView>

withDefaults(
  defineProps<{
    analytics: DashboardSnapshotView['analytics']
    analyticsError?: string
    healthTimeline: DashboardSnapshotView['healthTimeline']
  }>(),
  {
    analyticsError: '',
  },
)
</script>

<template>
  <div class="w-full">
    <p v-if="analyticsError" class="mb-4 text-z-sm text-z-warning-text" role="alert">
      {{ analytics ? '刷新失败，显示上次统计' : '统计暂不可用，请稍后重试' }}
    </p>

    <RequestHealthTimelineCard v-if="!analyticsError || analytics" :timeline="healthTimeline" />

    <slot name="requests" />
  </div>
</template>
