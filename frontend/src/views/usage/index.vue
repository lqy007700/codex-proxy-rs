<script setup lang="ts">
import type { TableColumn } from '@codex-proxy/ui'
import type { UsageListRecord } from '@/api'
import { defineTableColumns, useTableColumns, ZCard, ZDatePicker, ZIconButton, ZPageHeader, ZPagination, ZSegmented, ZTableColumnSettings } from '@codex-proxy/ui'

import { ChevronDown, CloudRain, Eye, List, Sun } from '@lucide/vue'
import { computed, shallowRef, watch } from 'vue'
import ProviderFilter from '@/components/ProviderFilter.vue'
import { usageRecordColumns } from '@/components/usage/shared/columns'
import UsageRecordsTable from '@/components/usage/UsageRecordsTable.vue'
import UsageErrorDetailModal from './components/UsageErrorDetailModal.vue'
import UsageFilters from './components/UsageFilters.vue'
import UsageInsightsGrid from './components/UsageInsightsGrid.vue'
import UsageOutcomeCell from './components/UsageOutcomeCell.vue'
import UsageRecordDetailModal from './components/UsageRecordDetailModal.vue'
import UsageSummaryCards from './components/UsageSummaryCards.vue'
import { useUsageRecordDetail } from './composables/useUsageRecordDetail'
import { useUsageRecordsTable } from './composables/useUsageRecordsTable'
import { useUsageTimeRange } from './composables/useUsageTimeRange'
import { isErrorUsageRecord } from './utils/detail'

const statsExpanded = shallowRef(true)
const requestColumnWidths: Record<string, number> = {
  clientApiKeyName: 112,
  createdAtDisplay: 176,
  accountEmail: 184,
  accountPlanType: 72,
  provider: 88,
  reasoningEffort: 104,
  route: 160,
  upstreamTransport: 80,
  clientTransport: 80,
  billing: 128,
  latency: 152,
  performance: 112,
  clientIp: 144,
  userAgent: 144,
  actions: 64,
}
const requestColumns = defineTableColumns<UsageListRecord>([
  { key: 'logicalOutcome', label: '请求结果', kind: 'custom', minWidth: 128, hideable: false },
  ...usageRecordColumns.map(column => ({
    ...column,
    minWidth: requestColumnWidths[column.key] ?? column.minWidth,
  })),
])
const recordViewOptions = [
  { label: '所有请求', value: 'all', icon: List },
  { label: '成功', value: 'success', icon: Sun },
  { label: '错误', value: 'error', icon: CloudRain },
]
const { timeRange, timeRangeParams, disabledDate, refreshTimeRangeEnd, latestTimeRangeParams }
  = useUsageTimeRange()

const {
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

const recordColumns = computed<TableColumn<UsageListRecord>[]>(() => recordScope.value === 'error'
  ? requestColumns.flatMap(column => column.key === 'route'
      ? [column, { key: 'errorMessage', label: '错误', kind: 'custom', size: '4xl', hideable: false }]
      : [column])
  : requestColumns)
const { visibleColumns, columnOptions, setColumnVisible, setColumnOrder, resetColumns } = useTableColumns(recordColumns, 'usage-requests')

const { showDetailModal, selectedUsageRecord, handleViewDetail } = useUsageRecordDetail()
const detailComponent = computed(() => isErrorUsageRecord(selectedUsageRecord.value)
  ? UsageErrorDetailModal
  : UsageRecordDetailModal)

watch(timeRange, () => {
  refreshTimeRangeEnd()
  void loadUsageRecords()
})
</script>

<template>
  <div class="w-full">
    <ZPageHeader title="使用统计" description="查看请求用量、性能趋势与调用错误记录">
      <template #actions>
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
        <ProviderFilter
          v-model="providerQuery"
          :disabled="refreshingList"
          class="shrink-0"
        />
      </template>
    </ZPageHeader>

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
          <ZSegmented v-model="recordScope" aria-label="请求结果筛选" :options="recordViewOptions" display="icon" class="w-30 shrink-0" />
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
              size="small"
              :columns="visibleColumns"
              :rows="records"
              :loading="loading"
              :empty-text="error ? `加载失败：${error}` : '当前筛选下暂无请求'"
            >
              <template #logicalOutcome="{ row }">
                <UsageOutcomeCell :record="row" :show-message="recordScope !== 'error'" />
              </template>
              <template #actions="{ row }">
                <div class="flex items-center justify-start">
                  <ZIconButton
                    size="small"
                    :aria-label="isErrorUsageRecord(row) ? '查看错误详情' : '查看使用记录详情'"
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

    <details :open="statsExpanded" class="group mt-5" @toggle="statsExpanded = ($event.target as HTMLDetailsElement).open">
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

    <component :is="detailComponent" v-model="showDetailModal" :record="selectedUsageRecord" />
  </div>
</template>
