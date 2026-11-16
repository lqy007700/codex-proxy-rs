<script setup lang="ts">
import { useDashboardSummary } from '@/composables/useDashboardSummary'
import DashboardContent from './components/DashboardContent.vue'
import RequestRecordsCard from './components/RequestRecordsCard.vue'

const {
  analytics,
  analyticsError,
  healthTimeline,
} = useDashboardSummary('overview')
</script>

<template>
  <DashboardContent
    :analytics="analytics"
    :analytics-error="analyticsError"
    :health-timeline="healthTimeline"
  >
    <template #requests>
      <RequestRecordsCard class="mt-6" />
    </template>
  </DashboardContent>
</template>
