import type { Ref } from 'vue'
import type { UsageErrorOrigin, UsageRecordScope } from '@/api'
import type { CallerUsageFilters } from '@/api/modules/usage'
import type { UsageTimeRangeParams } from '@/composables/useUsageTimeRange'
import { computed, onMounted, shallowRef, watch } from 'vue'
import { getUsageRecords } from '@/api'
import { useStablePagedQuery } from '@/composables/useStablePagedQuery'
import { withMinimumDuration } from '@/utils/operation'

interface UseUsageRecordsTableOptions {
  callerFilters: Readonly<Ref<CallerUsageFilters>>
  latestTimeRangeParams: () => UsageTimeRangeParams
}

export function useUsageRecordsTable(options: UseUsageRecordsTableOptions) {
  const searchQuery = shallowRef('')
  const search = computed(() => searchQuery.value.trim() || undefined)
  const accountSearchQuery = shallowRef('')
  const accountSearch = computed(() => accountSearchQuery.value.trim() || undefined)
  const errorOrigin = shallowRef<UsageErrorOrigin | ''>('')
  const providerQuery = shallowRef('')
  const recordScope = shallowRef<UsageRecordScope>('all')
  let tableParams = snapshot()
  const table = useStablePagedQuery({
    initialPageSize: 10,
    load: (pagination, requestOptions) => getUsageRecords({
      ...pagination,
      ...tableParams,
    }, requestOptions),
  })
  const { currentPage, pageSize, loading, error, items: records, total: totalRecords } = table
  const refreshingList = shallowRef(false)
  const usagePagination = computed(() => ({
    currentPage: currentPage.value,
    pageSize: pageSize.value,
    total: totalRecords.value,
  }))

  function snapshot() {
    return {
      ...options.latestTimeRangeParams(),
      ...options.callerFilters.value,
      provider: providerQuery.value || undefined,
      search: search.value,
      accountSearch: accountSearch.value,
      errorOrigin: recordScope.value === 'error'
        ? errorOrigin.value || undefined
        : undefined,
      scope: recordScope.value,
    }
  }

  function loadUsageRecords() {
    tableParams = snapshot()
    // 筛选范围变化后不能继续展示上一范围的数据，即使新查询失败
    return table.reloadFromStart()
  }

  async function refreshUsageRecords() {
    if (refreshingList.value || loading.value)
      return
    refreshingList.value = true
    try {
      await withMinimumDuration(loadUsageRecords)
    }
    finally {
      refreshingList.value = false
    }
  }

  function handlePageChange(nextPage: number) {
    if (tableParams.search !== search.value || tableParams.accountSearch !== accountSearch.value) {
      void loadUsageRecords()
      return
    }
    void table.execute(nextPage)
  }

  function handlePageSizeChange(nextPageSize: number) {
    pageSize.value = nextPageSize
    if (tableParams.search !== search.value || tableParams.accountSearch !== accountSearch.value) {
      void loadUsageRecords()
      return
    }
    void table.reloadFromStart()
  }

  onMounted(() => {
    void loadUsageRecords()
  })
  watch([
    options.callerFilters,
    providerQuery,
    recordScope,
    errorOrigin,
  ], () => {
    void loadUsageRecords()
  })
  watch([search, accountSearch], (_value, _previous, onCleanup) => {
    const timer = setTimeout(() => {
      if (tableParams.search !== search.value || tableParams.accountSearch !== accountSearch.value)
        void loadUsageRecords()
    }, 250)
    onCleanup(() => clearTimeout(timer))
  })

  return {
    searchQuery,
    accountSearchQuery,
    errorOrigin,
    providerQuery,
    recordScope,
    usagePagination,
    loading,
    error,
    records,
    refreshingList,
    loadUsageRecords,
    refreshUsageRecords,
    handlePageChange,
    handlePageSizeChange,
  }
}
