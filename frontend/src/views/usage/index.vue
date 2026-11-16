<script setup lang="ts">
import { ZButton, ZCard, ZDatePicker, ZIconButton } from '@codex-proxy/ui'
import { ChevronDown, RefreshCw } from '@lucide/vue'
import { shallowRef, watch } from 'vue'
import { useRouter } from 'vue-router'
import AppPageHeader from '@/components/AppPageHeader.vue'
import ProviderFilter from '@/components/ProviderFilter.vue'
import { useUsageCallerScope } from '@/composables/useUsageCallerScope'
import { useUsageTimeRange } from '@/composables/useUsageTimeRange'
import UsageDailyOverview from './components/UsageDailyOverview.vue'
import UsageInsightsGrid from './components/UsageInsightsGrid.vue'
import UsageSummaryCards from './components/UsageSummaryCards.vue'
import { useUsageAnalytics } from './composables/useUsageAnalytics'

const router = useRouter()
const statsExpanded = shallowRef(true)
const { callerFilters, scopedLabels } = useUsageCallerScope()
const { timeRange, timeRangeParams, disabledDate, refreshTimeRangeEnd } = useUsageTimeRange()
const {
  analyticsLoading,
  summaryLoading,
  overviewLoading,
  diagnosticsLoading,
  summary,
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
    <AppPageHeader title="使用统计" description="查看当日概况、请求用量与性能趋势" />

    <UsageDailyOverview />

    <div class="mt-6 flex flex-wrap items-center justify-end gap-2" role="group" aria-label="统计分析筛选">
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
        aria-label="刷新使用统计"
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

    <div v-if="scopedLabels.length" class="mt-4 flex flex-wrap items-center gap-3 text-z-sm text-z-text-secondary">
      <span class="break-all">当前限定：{{ scopedLabels.join(' · ') }}</span>
      <ZButton size="small" @click="router.replace({ path: '/usage' })">
        清除限定
      </ZButton>
    </div>

    <details :open="statsExpanded" class="group mt-5" @toggle="statsExpanded = ($event.target as HTMLDetailsElement).open">
      <ZCard
        as="summary"
        class="flex cursor-pointer list-none items-center justify-between gap-3 select-none focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-z-primary [&::-webkit-details-marker]:hidden"
      >
        <span class="text-xl leading-[1.15] font-heavy text-z-text">统计概览</span>
        <span class="flex shrink-0 items-center gap-2 text-z-sm font-emphasis text-z-text-secondary">
          <span class="group-open:hidden">展开</span>
          <span class="hidden group-open:inline">收起</span>
          <ChevronDown class="size-4.5 transition-transform group-open:rotate-180 motion-reduce:transition-none" />
        </span>
      </ZCard>
      <template v-if="statsExpanded">
        <UsageSummaryCards :summary="summary" :loading="summaryLoading" />
        <UsageInsightsGrid
          v-model:diagnostic-dimension="diagnosticDimension"
          :overview="insights.overview"
          :diagnostics="insights.diagnostics"
          :overview-loading="overviewLoading"
          :diagnostics-loading="diagnosticsLoading"
        />
      </template>
    </details>
  </div>
</template>
