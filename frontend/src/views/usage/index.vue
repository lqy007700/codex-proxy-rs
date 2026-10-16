<script setup lang="ts">
import type { UsageListRecord } from '@/api'
import { defineTableColumns, useTableColumns, ZCard, ZIconButton, ZPageHeader, ZPagination, ZSegmented, ZSelect, ZTableColumnSettings } from '@codex-proxy/ui'

import { ChevronDown, CloudRain, Eye, List, Sun } from '@lucide/vue'
import { shallowRef, watch } from 'vue'
import ProviderFilter from '@/components/ProviderFilter.vue'
import { usageRecordColumns } from '@/components/usage/shared/columns'
import UsageRecordsTable from '@/components/usage/UsageRecordsTable.vue'
import UsageFilters from './components/UsageFilters.vue'
import UsageInsightsGrid from './components/UsageInsightsGrid.vue'
import UsageOutcomeCell from './components/UsageOutcomeCell.vue'
import UsageRecordDetailModal from './components/UsageRecordDetailModal.vue'
import UsageSummaryCards from './components/UsageSummaryCards.vue'
import { useUsageRecordDetail } from './composables/useUsageRecordDetail'
import { useUsageRecordsTable } from './composables/useUsageRecordsTable'
import { useUsageTimeRange } from './composables/useUsageTimeRange'
import { usageTimeRangeOptions } from './constants'

const statsExpanded = shallowRef(false)
const requestColumns = defineTableColumns<UsageListRecord>([
  { key: 'logicalOutcome', label: '请求结果', kind: 'custom', size: 'lg', hideable: false },
  ...usageRecordColumns,
])
const { visibleColumns, columnOptions, setColumnVisible, setColumnOrder, resetColumns } = useTableColumns(requestColumns, 'usage-requests')
const recordViewOptions = [
  { label: '所有请求', value: 'all', icon: List },
  { label: '成功', value: 'success', icon: Sun },
  { label: '错误', value: 'error', icon: CloudRain },
]
const { timeRange, timeRangeParams, refreshTimeRangeEnd, latestTimeRangeParams }
  = useUsageTimeRange()

const {
  currentPage,
  searchQuery,
  recordScope,
  providerQuery,
  usagePagination,
  loading,
  error,
  analyticsLoading,
  records,
  summary,
  insights,
  refreshingList,
  diagnosticDimension,
  loadUsageRecords,
  refreshUsageRecords,
  handlePageChange,
  handlePageSizeChange,
} = useUsageRecordsTable({
  timeRangeParams,
  latestTimeRangeParams,
})

const { showDetailModal, selectedUsageRecord, handleViewDetail } = useUsageRecordDetail()

watch(timeRange, () => {
  refreshTimeRangeEnd()
  currentPage.value = 1
  void loadUsageRecords()
})
</script>

<template>
  <div class="w-full">
    <ZPageHeader title="使用统计" description="查看请求用量、性能趋势与调用错误记录">
      <template #actions>
        <ZSelect v-model="timeRange" :options="usageTimeRangeOptions" class="w-34" />
        <ProviderFilter
          v-model="providerQuery"
          :disabled="refreshingList"
          class="shrink-0"
        />
      </template>
    </ZPageHeader>

    <details class="group mt-5" @toggle="statsExpanded = ($event.target as HTMLDetailsElement).open">
      <ZCard
        as="summary"
        class="flex cursor-pointer list-none items-center justify-between gap-3 select-none focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-cp-primary [&::-webkit-details-marker]:hidden"
      >
        <span class="text-xl leading-[1.15] font-heavy text-cp-text">统计概览</span>
        <span class="flex shrink-0 items-center gap-2 text-cp-sm font-emphasis text-cp-text-secondary">
          <span class="group-open:hidden">展开</span>
          <span class="hidden group-open:inline">收起</span>
          <ChevronDown class="size-4.5 transition-transform group-open:rotate-180 motion-reduce:transition-none" />
        </span>
      </ZCard>
      <template v-if="statsExpanded">
        <UsageSummaryCards :summary="summary" />
        <UsageInsightsGrid
          v-model:diagnostic-dimension="diagnosticDimension"
          :overview="insights.overview"
          :diagnostics="insights.diagnostics"
          :loading="analyticsLoading"
        />
      </template>
    </details>

    <ZCard
      class="mt-5 flex flex-col"
    >
      <template #header>
        <div class="flex flex-wrap items-center justify-between gap-3">
          <div>
            <h2 class="m-0 text-xl leading-[1.15] font-heavy text-cp-text">
              请求明细
            </h2>
            <p
              class="mt-1.75 mb-0 text-cp leading-[1.15] font-emphasis text-cp-text-secondary"
            >
              查看请求结果、用量与错误详情
            </p>
          </div>
          <ZSegmented v-model="recordScope" aria-label="请求结果筛选" :options="recordViewOptions" class="w-72 max-w-full" />
        </div>
      </template>

      <template #body>
        <div
          class="grid min-h-130 min-w-0 flex-1 grid-rows-[auto_minmax(0,1fr)] gap-3"
        >
          <UsageFilters
            v-model:search="searchQuery"
            :loading="loading"
            :refreshing="refreshingList"
            @refresh="refreshUsageRecords"
          >
            <template #actions>
              <ZTableColumnSettings
                :options="columnOptions"
                @change="setColumnVisible"
                @reorder="setColumnOrder"
                @reset="resetColumns"
              />
            </template>
          </UsageFilters>

          <div class="flex min-h-0 min-w-0 flex-col">
            <UsageRecordsTable
              class="min-h-0 flex-1"
              :columns="visibleColumns"
              :rows="records"
              :loading="loading"
              :empty-text="error ? `加载失败：${error}` : '当前筛选下暂无请求'"
            >
              <template #logicalOutcome="{ row }">
                <UsageOutcomeCell :record="row" />
              </template>
              <template #actions="{ row }">
                <div class="flex items-center justify-start">
                  <ZIconButton
                    size="small"
                    aria-label="查看使用记录详情"
                    @click="handleViewDetail(row)"
                  >
                    <Eye class="size-3.5" />
                  </ZIconButton>
                </div>
              </template>
            </UsageRecordsTable>
            <ZPagination
              v-bind="usagePagination"
              :disabled="loading"
              @current-change="handlePageChange"
              @size-change="handlePageSizeChange"
            />
          </div>
        </div>
      </template>
    </ZCard>

    <UsageRecordDetailModal v-model="showDetailModal" :record="selectedUsageRecord" />
  </div>
</template>
