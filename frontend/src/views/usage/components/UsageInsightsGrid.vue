<script setup lang="ts">
import type {
  getUsageRecordInsightsDiagnostics,
  getUsageRecordInsightsOverview,
} from '@/api'

import UsageCostCard from './UsageCostCard.vue'
import UsageDiagnosticCard from './UsageDiagnosticCard.vue'
import UsageHealthCard from './UsageHealthCard.vue'
import UsagePerformanceCard from './UsagePerformanceCard.vue'

withDefaults(
  defineProps<{
    overview: Omit<Awaited<ReturnType<typeof getUsageRecordInsightsOverview>>, 'analytics'>
    diagnostics: Omit<Awaited<ReturnType<typeof getUsageRecordInsightsDiagnostics>>, 'analytics'>
    overviewLoading?: boolean
    diagnosticsLoading?: boolean
  }>(),
  {
    overviewLoading: false,
    diagnosticsLoading: false,
  },
)

const diagnosticDimension = defineModel('diagnosticDimension', {
  type: String,
  default: 'model',
})
</script>

<template>
  <section
    class="mt-5 grid grid-cols-1 gap-3 xl:auto-rows-97 xl:grid-cols-2"
    aria-label="使用统计观测"
  >
    <UsageHealthCard
      :health="overview.health"
      :granularity="overview.granularity"
      :loading="overviewLoading"
    />

    <UsageDiagnosticCard
      v-model:dimension="diagnosticDimension"
      :diagnostics="diagnostics"
      :loading="diagnosticsLoading"
    />

    <UsagePerformanceCard
      :performance="overview.performance"
      :activity="overview.health.points"
      :loading="overviewLoading"
    />

    <UsageCostCard
      :cost="overview.cost"
      :activity="overview.health.points"
      :loading="overviewLoading"
    />
  </section>
</template>
