<script setup lang="ts">
import type { TableColumn } from '@codex-proxy/ui'
import type { UsageListRecord } from '@/api'
import { defineTableColumns, useTableColumns, ZButton, ZCard, ZDatePicker, ZIconButton, ZInput, ZPagination, ZSegmented, ZTableColumnSettings } from '@codex-proxy/ui'
import { CloudRain, Eye, List, Sun } from '@lucide/vue'

import { computed, shallowRef, watch } from 'vue'
import { useRouter } from 'vue-router'
import ProviderFilter from '@/components/ProviderFilter.vue'
import { usageRecordColumns } from '@/components/usage/shared/columns'
import UsageRecordsTable from '@/components/usage/UsageRecordsTable.vue'
import { useUsageCallerScope } from '@/composables/useUsageCallerScope'
import { useUsageTimeRange } from '@/composables/useUsageTimeRange'
import { useUsageRecordDetail } from '../composables/useUsageRecordDetail'
import { useUsageRecordsTable } from '../composables/useUsageRecordsTable'
import { isErrorUsageRecord } from '../utils/detail'
import UsageAccountAttemptsCell from './UsageAccountAttemptsCell.vue'
import UsageAffinityModal from './UsageAffinityModal.vue'
import UsageErrorDetailModal from './UsageErrorDetailModal.vue'
import UsageFilters from './UsageFilters.vue'
import UsageOutcomeCell from './UsageOutcomeCell.vue'
import UsageRecordDetailModal from './UsageRecordDetailModal.vue'

const router = useRouter()
const { callerFilters, scopedLabels } = useUsageCallerScope()
const affinityRecord = shallowRef<UsageListRecord | null>(null)
const showAffinityModal = shallowRef(false)
function editAffinity(record: UsageListRecord) {
  affinityRecord.value = record
  showAffinityModal.value = true
}
const requestColumns = defineTableColumns<UsageListRecord>([
  {
    key: 'logicalOutcome',
    label: '请求结果',
    kind: 'custom',
    size: 'lg',
    hideable: false,
  },
  ...usageRecordColumns,
])
const recordViewOptions = [
  { label: '所有请求', value: 'all', icon: List },
  { label: '成功', value: 'success', icon: Sun },
  { label: '错误', value: 'error', icon: CloudRain },
]
const { timeRange, disabledDate, latestTimeRangeParams }
  = useUsageTimeRange()

const {
  searchQuery,
  recordScope,
  providerQuery,
  usagePagination,
  loading,
  error,
  records,
  refreshingList,
  loadUsageRecords,
  refreshUsageRecords,
  handlePageChange,
  handlePageSizeChange,
} = useUsageRecordsTable({
  callerFilters,
  latestTimeRangeParams,
})

const jumpPage = shallowRef('1')
const totalPages = computed(() => Math.max(1, Math.ceil(usagePagination.value.total / usagePagination.value.pageSize)))
watch(usagePagination, ({ currentPage }) => {
  jumpPage.value = String(currentPage)
})
function jumpToPage() {
  const page = Number(jumpPage.value)
  if (loading.value || !Number.isInteger(page) || page < 1 || page > totalPages.value || page === usagePagination.value.currentPage)
    return
  handlePageChange(page)
}

const recordColumns = computed<TableColumn<UsageListRecord>[]>(() => recordScope.value === 'error'
  ? requestColumns.flatMap(column => column.key === 'route'
      ? [
          column,
          {
            key: 'errorMessage',
            label: '错误',
            kind: 'custom',
            size: '4xl',
            hideable: false,
          },
        ]
      : [column])
  : requestColumns)
const { visibleColumns, columnOptions, setColumnVisible, setColumnOrder, resetColumns } = useTableColumns(recordColumns, 'usage-requests')

const { showDetailModal, selectedUsageRecord, handleViewDetail, detailIsError, detailLoading, detailError, reloadDetail } = useUsageRecordDetail()
const detailComponent = computed(() => detailIsError.value
  ? UsageErrorDetailModal
  : UsageRecordDetailModal)

watch(timeRange, () => {
  void loadUsageRecords()
})
</script>

<template>
  <section class="@container w-full" aria-label="请求明细">
    <ZCard
      class="flex flex-col"
    >
      <template #header>
        <div class="flex flex-wrap items-center justify-between gap-3">
          <div>
            <h2 class="m-0 text-xl leading-[1.15] font-heavy text-z-text">
              请求明细
            </h2>
            <p
              class="mt-1.75 mb-0 text-z leading-[1.15] font-emphasis text-z-text-secondary"
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
          <div class="space-y-3">
            <div v-if="scopedLabels.length" class="flex flex-wrap items-center gap-3 text-z-sm text-z-text-secondary">
              <span class="break-all">当前限定：{{ scopedLabels.join(' · ') }}</span>
              <ZButton size="small" @click="router.replace({ path: '/' })">
                清除限定
              </ZButton>
            </div>
            <div class="flex flex-wrap items-center gap-2">
              <ZDatePicker
                v-model="timeRange"
                type="daterange"
                :clearable="false"
                :editable="false"
                :disabled-date="disabledDate"
                single-panel
                aria-label="请求日期范围，仅可选择最近 31 天"
                class="w-60 max-w-full"
              />
              <ProviderFilter v-model="providerQuery" :disabled="refreshingList" />
            </div>

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
          </div>

          <div class="flex min-h-0 min-w-0 flex-col">
            <UsageRecordsTable
              class="min-h-0 flex-1"
              :columns="visibleColumns"
              :rows="records"
              :loading="loading"
              :empty-text="error ? `加载失败：${error}` : '当前筛选下暂无请求'"
            >
              <template #logicalOutcome="{ row }">
                <UsageOutcomeCell :record="row" :show-message="recordScope !== 'error'" />
              </template>
              <template #accountEmail="{ row }">
                <UsageAccountAttemptsCell :record="row" />
              </template>
              <template #callerTokenName="{ row, displayValue }">
                <button
                  v-if="row.callerIdentity?.mode === 'newApi' && row.provider"
                  type="button"
                  class="block max-w-full cursor-pointer truncate rounded-z-sm border-0 bg-transparent p-0 text-left font-mono text-z-sm leading-none font-bold text-z-primary-text outline-none hover:underline focus-visible:ring-2 focus-visible:ring-z-control-outline"
                  :title="row.callerTokenName || row.callerIdentity.tokenId"
                  :aria-label="`调整 ${row.callerTokenName || row.callerIdentity.tokenId} 的会话亲和`"
                  @click="editAffinity(row)"
                >
                  {{ row.callerTokenName || row.callerIdentity.tokenId }}
                </button>
                <span v-else class="block max-w-full truncate font-mono text-z-sm leading-none font-bold text-z-text" :title="String(displayValue || '')">
                  {{ displayValue || '—' }}
                </span>
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
            <div class="flex flex-wrap items-center gap-x-4">
              <ZPagination
                v-bind="usagePagination"
                class="min-w-0 flex-1"
                :disabled="loading"
                @current-change="handlePageChange"
                @size-change="handlePageSizeChange"
              />
              <form class="ml-auto mt-2 flex shrink-0 items-center gap-2 text-z-sm text-z-text-secondary" @submit.prevent="jumpToPage">
                <span>共 {{ totalPages }} 页，跳至</span>
                <ZInput
                  v-model="jumpPage"
                  type="number"
                  inputmode="numeric"
                  min="1"
                  :max="totalPages"
                  step="1"
                  required
                  size="small"
                  aria-label="跳转页码"
                  class="w-20"
                  :disabled="loading || !usagePagination.total"
                />
                <span>页</span>
                <ZButton native-type="submit" size="small" :disabled="loading || !usagePagination.total">
                  跳转
                </ZButton>
              </form>
            </div>
          </div>
        </div>
      </template>
    </ZCard>
    <UsageAffinityModal v-model="showAffinityModal" :record="affinityRecord" />
    <component :is="detailComponent" v-model="showDetailModal" :record="selectedUsageRecord" :loading="detailLoading" :error="detailError" @retry="reloadDetail" />
  </section>
</template>
