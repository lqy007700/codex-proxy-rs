<script setup lang="ts">
import { ZButton, ZDatePicker, ZIconButton } from '@codex-proxy/ui'
import { RefreshCw } from '@lucide/vue'
import { watch } from 'vue'
import { useRouter } from 'vue-router'
import AppPageHeader from '@/components/AppPageHeader.vue'
import ProviderFilter from '@/components/ProviderFilter.vue'
import SummaryHeartbeat from '@/components/usage/SummaryHeartbeat.vue'
import { useDashboardSummary } from '@/composables/useDashboardSummary'
import { useUsageCallerScope } from '@/composables/useUsageCallerScope'
import { useUsageTimeRange } from '@/composables/useUsageTimeRange'
import AccountOverviewCard from './components/AccountOverviewCard.vue'
import MetricCard from './components/MetricCard.vue'
import RequestTrendCard from './components/RequestTrendCard.vue'
import UsageInsightsGrid from './components/UsageInsightsGrid.vue'
import WireProfileCard from './components/WireProfileCard.vue'
import { useUsageAnalytics } from './composables/useUsageAnalytics'

const router = useRouter()
const { callerFilters, scopedLabels } = useUsageCallerScope()
const { timeRange, timeRangeParams, disabledDate, refreshTimeRangeEnd } = useUsageTimeRange()
const {
  loading: dailyLoading,
  refreshing: dailyRefreshing,
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
  refresh: refreshDaily,
  loadTrend,
} = useDashboardSummary('usage')
const {
  analyticsLoading,
  overviewLoading,
  diagnosticsLoading,
  insights,
  providerQuery,
  diagnosticDimension,
  refreshing,
  loadUsageAnalytics,
  refreshUsageAnalytics,
} = useUsageAnalytics({ timeRangeParams, callerFilters, refreshTimeRangeEnd })

watch(timeRange, () => {
  void loadUsageAnalytics()
})
</script>

<template>
  <div class="@container w-full">
    <AppPageHeader title="使用统计" description="查看请求、账号运行状态与性能成本" />

    <div class="mt-6 flex flex-wrap items-center justify-between gap-3">
      <span class="text-z-sm font-emphasis text-z-text-secondary">今日 · 全部平台</span>
      <div class="flex items-center gap-3 text-z-sm text-z-text-secondary">
        <SummaryHeartbeat :updated-at="lastRefreshedAt" />
        <ZIconButton
          aria-label="刷新当日统计与账号状态"
          :loading="dailyLoading || dailyRefreshing"
          :disabled="dailyLoading || dailyRefreshing"
          @click="refreshDaily"
        >
          <template #loading>
            <RefreshCw class="size-4.5 animate-spin motion-reduce:animate-none" />
          </template>
          <RefreshCw class="size-4.5" />
        </ZIconButton>
      </div>
    </div>

    <p v-if="analyticsError" class="mt-4 text-z-sm text-z-warning-text" role="alert">
      {{ analytics ? '刷新失败，显示上次统计' : '当日统计与账号状态暂不可用，请稍后重试' }}
    </p>

    <template v-if="!analyticsError || analytics">
      <div class="mt-4 grid grid-cols-1 gap-4 lg:grid-cols-3" role="group" aria-label="当日全局指标">
        <MetricCard v-for="metric in metrics" :key="metric.title" :metric="metric" :loading="dailyLoading && !analytics" />
      </div>
      <AccountOverviewCard
        :accounts="accountUsage"
        :pool="poolSummary"
        :capacity="capacityInfo"
        :rotation-strategy="rotationStrategy"
        class="mt-5"
      />
    </template>

    <div class="mt-6 flex flex-wrap items-center justify-between gap-3">
      <span class="text-z-sm text-z-text-secondary">以下分析按所选日期与平台统计</span>
      <div class="flex flex-wrap items-center gap-2" role="group" aria-label="统计分析筛选">
        <ZDatePicker
          v-model="timeRange"
          type="daterange"
          :clearable="false"
          :editable="false"
          :disabled-date="disabledDate"
          single-panel
          aria-label="统计日期范围，仅可选择最近 31 天"
          class="w-60 max-w-full"
        />
        <ProviderFilter v-model="providerQuery" :disabled="refreshing" class="shrink-0" />
        <ZIconButton
          aria-label="刷新请求分析"
          :loading="refreshing"
          :disabled="analyticsLoading || refreshing"
          @click="refreshUsageAnalytics"
        >
          <template #loading>
            <RefreshCw class="size-4.5 animate-spin motion-reduce:animate-none" />
          </template>
          <RefreshCw class="size-4.5" />
        </ZIconButton>
      </div>
    </div>

    <div v-if="scopedLabels.length" class="mt-4 flex flex-wrap items-center gap-3 text-z-sm text-z-text-secondary">
      <span class="break-all">当前分析限定：{{ scopedLabels.join(' · ') }}</span>
      <ZButton size="small" @click="router.replace({ path: '/usage' })">
        清除限定
      </ZButton>
    </div>

    <UsageInsightsGrid
      v-model:diagnostic-dimension="diagnosticDimension"
      :overview="insights.overview"
      :diagnostics="insights.diagnostics"
      :overview-loading="overviewLoading"
      :diagnostics-loading="diagnosticsLoading"
    />

    <template v-if="!analyticsError || analytics">
      <RequestTrendCard
        v-model:kind="activeTrendKind"
        :points="trendPoints"
        :summary="trendSummary"
        :loading="trendLoading"
        :error="trendError"
        class="mt-5"
        @trend-change="loadTrend"
      />
      <WireProfileCard :profiles="wireProfiles" class="mt-5" />
    </template>
  </div>
</template>
