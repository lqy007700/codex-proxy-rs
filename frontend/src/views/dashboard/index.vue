<script setup lang="ts">
import { useDashboardSummary } from '@/composables/useDashboardSummary'
import DashboardContent from './components/DashboardContent.vue'
import RequestRecordsCard from './components/RequestRecordsCard.vue'

const {
  analytics,
  analyticsError,
  healthTimeline,
  accountUsage,
  wireProfiles,
  poolSummary,
  capacityInfo,
  rotationStrategy,
} = useDashboardSummary()
</script>

<template>
  <DashboardContent
    :analytics="analytics"
    :analytics-error="analyticsError"
    :health-timeline="healthTimeline"
    :account-usage="accountUsage"
    :wire-profiles="wireProfiles"
    :pool-summary="poolSummary"
    :capacity-info="capacityInfo"
    :rotation-strategy="rotationStrategy"
  >
    <template #requests>
      <RequestRecordsCard class="mt-6" />
    </template>
  </DashboardContent>
</template>
