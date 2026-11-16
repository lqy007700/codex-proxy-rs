<script setup lang="ts">
import { ZIconButton } from '@codex-proxy/ui'
import { RefreshCw } from '@lucide/vue'
import SummaryHeartbeat from '@/components/usage/SummaryHeartbeat.vue'
import { useDashboardSummary } from '@/composables/useDashboardSummary'
import AccountOverviewCard from './AccountOverviewCard.vue'
import MetricCard from './MetricCard.vue'
import RequestTrendCard from './RequestTrendCard.vue'
import WireProfileCard from './WireProfileCard.vue'

const {
  loading,
  refreshing,
  activeTrendKind,
  metrics,
  analytics,
  analyticsError,
  accountUsage,
  wireProfiles,
  poolSummary,
  capacityInfo,
  rotationStrategy,
  trendPoints,
  trendSummary,
  trendLoading,
  trendError,
  lastRefreshedAt,
  refresh,
  loadTrend,
} = useDashboardSummary('usage')
</script>

<template>
  <section class="mt-6" aria-label="当日全局统计">
    <div class="flex flex-wrap items-center justify-between gap-3">
      <h2 class="m-0 text-xl font-heavy text-z-text">
        当日全局统计
      </h2>
      <div class="flex items-center gap-3">
        <SummaryHeartbeat :updated-at="lastRefreshedAt" />
        <ZIconButton
          aria-label="刷新当日统计"
          :loading="loading || refreshing"
          :disabled="loading || refreshing"
          @click="refresh"
        >
          <template #loading>
            <RefreshCw class="size-4.5 animate-spin motion-reduce:animate-none" />
          </template>
          <RefreshCw class="size-4.5" />
        </ZIconButton>
      </div>
    </div>

    <p v-if="analyticsError" class="mt-4 text-z-sm text-z-warning-text" role="alert">
      {{ analytics ? '刷新失败，显示上次统计' : '统计暂不可用，请稍后重试' }}
    </p>

    <template v-if="!analyticsError || analytics">
      <div class="mt-4 grid grid-cols-1 gap-4 md:grid-cols-2 2xl:grid-cols-4 2xl:gap-6" role="group" aria-label="核心指标">
        <MetricCard v-for="metric in metrics" :key="metric.title" :metric="metric" />
      </div>
      <RequestTrendCard
        v-model:kind="activeTrendKind"
        :points="trendPoints"
        :summary="trendSummary"
        :loading="trendLoading"
        :error="trendError"
        class="mt-6"
        @trend-change="loadTrend"
      />
      <AccountOverviewCard
        :accounts="accountUsage"
        :pool="poolSummary"
        :capacity="capacityInfo"
        :rotation-strategy="rotationStrategy"
        class="mt-6"
      />
      <WireProfileCard :profiles="wireProfiles" class="mt-6" />
    </template>
  </section>
</template>
