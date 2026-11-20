<script setup lang="ts">
import type { AccountModelTestPreview as Preview } from '../utils/modelTestPreview'
import type { Account, AccountModelTestDetail, AccountModelTestSummary } from '@/api'
import { defineTableColumns, ZButton, ZCard, ZDialog, ZEmpty, ZPagination, ZSelect, ZTable, ZTag } from '@codex-proxy/ui'
import { useIntervalFn } from '@vueuse/core'
import { computed, ref, shallowRef, watch } from 'vue'

import { getAccountDetail, getAccountModelTestDetail, getAccountModelTests, getAccounts } from '@/api'
import { usePagedQuery } from '@/composables/usePagedQuery'
import { useRequestState } from '@/composables/useRequestState'
import { accountCompactIdentityLabel } from '@/utils/accountIdentity'
import { formatDuration } from '@/utils/format'
import AccountModelTestPreview from './AccountModelTestPreview.vue'
import AccountModelTestResult from './AccountModelTestResult.vue'
import AccountModelTestThumbnail from './AccountModelTestThumbnail.vue'

const { accountName, submitted } = defineProps<{ accountName: string, submitted: number }>()
const emit = defineEmits<{ rerun: [account: Account, record: AccountModelTestDetail] }>()
const accountId = defineModel<string>('accountId')
const columns = defineTableColumns<AccountModelTestSummary>([
  { key: 'startedAtDisplay', label: '测试时间', width: 178 },
  { key: 'accountName', label: '账号', width: 190 },
  { key: 'modelId', label: '上游模型 ID', minWidth: 190 },
  { key: 'kind', label: '类型', width: 104 },
  { key: 'status', label: '状态', width: 106 },
  { key: 'durationMs', label: '耗时', width: 90 },
  { key: 'preview', label: '结果预览', width: 152 },
  { key: 'actions', label: '操作', width: 112 },
])
const kinds = { text: '文本', image: '图片', animation: 'SVG 动画' }
const accountOptions = shallowRef<{ label: string, value: string }[]>([])
const optionsRequest = useRequestState()
const accountFilterOptions = computed(() => {
  const id = accountId.value
  if (!id || accountOptions.value.some(option => option.value === id))
    return accountOptions.value
  return [
    { label: accountName || id, value: id },
    ...accountOptions.value,
  ]
})
const { page, pageSize, total, items, loading, error, execute, reloadFromStart } = usePagedQuery({
  initialPageSize: 20,
  load: ({ page }, options) => getAccountModelTests({ page, accountId: accountId.value || undefined }, options),
})
const showDetail = ref(false)
const showPreview = ref(false)
const enlargedPreview = shallowRef<Preview>()

function enlargePreview(preview: Preview) {
  enlargedPreview.value = preview
  showPreview.value = true
}

const selectedId = ref('')
const detail = shallowRef<AccountModelTestDetail>()
const detailRequest = useRequestState()
const editRequest = useRequestState()

async function loadAccounts(search = '') {
  const requestId = optionsRequest.start()
  try {
    const result = await getAccounts({ page: 1, pageSize: 30, search: search.trim() || undefined }, { signal: optionsRequest.signal, silent: true })
    if (optionsRequest.isCurrent(requestId)) {
      accountOptions.value = result.items.map(account => ({
        label: accountCompactIdentityLabel(account),
        value: account.id,
      }))
    }
  }
  catch (cause) {
    optionsRequest.fail(requestId, cause)
  }
  finally {
    optionsRequest.finish(requestId)
  }
}

async function loadDetail(testId: string, background = false) {
  const requestId = detailRequest.start(background)
  try {
    const result = await getAccountModelTestDetail(testId, { signal: detailRequest.signal, silent: true })
    if (detailRequest.isCurrent(requestId))
      detail.value = result
  }
  catch (cause) {
    detailRequest.fail(requestId, cause, background)
  }
  finally {
    detailRequest.finish(requestId)
  }
}

function openDetail(testId: string) {
  selectedId.value = testId
  detail.value = undefined
  showDetail.value = true
  void loadDetail(testId)
}

async function rerun() {
  const record = detail.value
  if (!record || editRequest.loading.value)
    return
  const requestId = editRequest.start()
  try {
    const { account } = await getAccountDetail({ accountId: record.accountId }, { signal: editRequest.signal, silent: true })
    if (!editRequest.isCurrent(requestId))
      return
    showDetail.value = false
    emit('rerun', account, record)
  }
  catch (cause) {
    editRequest.fail(requestId, cause)
  }
  finally {
    editRequest.finish(requestId)
  }
}

watch(() => accountId.value, () => void reloadFromStart(), { immediate: true })
watch(() => submitted, () => void reloadFromStart())
watch(showDetail, (open) => {
  if (!open) {
    detailRequest.invalidate()
    editRequest.invalidate()
  }
})
useIntervalFn(() => {
  if (!loading.value && items.value.some(item => item.status === 'running'))
    void execute({ silent: true })
  if (showDetail.value && detail.value?.status === 'running' && !detailRequest.loading.value)
    void loadDetail(selectedId.value, true)
}, 4000)
void loadAccounts()
</script>

<template>
  <ZCard class="mt-4 flex flex-col xl:min-h-0 xl:flex-1">
    <template #header>
      <div class="flex flex-wrap items-center gap-3">
        <ZSelect
          v-model="accountId"
          class="w-64 max-w-full"
          placeholder="全部账号"
          aria-label="按账号筛选"
          :options="accountFilterOptions"
          :loading="optionsRequest.loading.value"
          clearable
          filterable
          remote
          @query-change="loadAccounts"
        />
        <span class="text-z-sm text-z-text-secondary">每个账号保留最近 10 次已完成的测试</span>
        <ZButton class="ml-auto" :loading="loading" @click="execute()">
          刷新
        </ZButton>
      </div>
    </template>
    <div v-if="error || optionsRequest.error.value" class="mb-3 text-z-sm text-z-error-text" role="alert">
      {{ error || optionsRequest.error.value }}
    </div>
    <ZTable
      :columns="columns"
      :data="items"
      :loading="loading"
      row-key="id"
      class="min-h-0"
    >
      <template #empty>
        <ZEmpty description="暂无测试记录" />
      </template>
      <template #kind="{ row }">
        {{ kinds[row.kind] }}
      </template>
      <template #status="{ row }">
        <ZTag v-if="row.status === 'running'" type="info" size="small">
          测试中
        </ZTag>
        <ZTag v-else-if="row.status === 'failed'" type="danger" size="small">
          失败
        </ZTag>
        <ZTag v-else type="success" size="small">
          完成
        </ZTag>
      </template>
      <template #durationMs="{ row }">
        {{ formatDuration(row.durationMs) }}
      </template>
      <template #preview="{ row }">
        <AccountModelTestThumbnail :key="row.id" :record="row" @enlarge="enlargePreview" />
      </template>
      <template #actions="{ row }">
        <ZButton size="small" @click="openDetail(row.id)">
          查看详情
        </ZButton>
      </template>
    </ZTable>
    <ZPagination
      v-if="total > pageSize"
      :current-page="page"
      :page-size="pageSize"
      :page-sizes="[pageSize]"
      :total="total"
      :disabled="loading"
      @current-change="next => execute({ page: next })"
    />
  </ZCard>

  <ZDialog v-model="showPreview" title="结果预览" width="68rem">
    <AccountModelTestPreview v-if="showPreview && enlargedPreview" :preview="enlargedPreview" />
    <template #footer>
      <ZButton @click="showPreview = false">
        关闭
      </ZButton>
    </template>
  </ZDialog>

  <ZDialog v-model="showDetail" title="测试详情" width="46rem">
    <div v-if="detail" class="grid gap-3">
      <p class="m-0 text-z-sm text-z-text-secondary">
        {{ detail.accountName }} · {{ detail.startedAtDisplay }}
      </p>
      <div class="grid gap-2">
        <span class="text-z-sm font-heavy text-z-text-secondary">本次提示词</span>
        <pre class="m-0 max-h-48 overflow-auto whitespace-pre-wrap wrap-anywhere rounded-z bg-z-fill-quaternary p-3 text-z-sm" v-text="detail.prompt" />
      </div>
      <AccountModelTestResult :result="detail" />
    </div>
    <p v-else-if="detailRequest.loading.value" class="m-0 text-z-sm text-z-text-secondary" role="status">
      正在加载测试详情
    </p>
    <p v-if="detailRequest.error.value || editRequest.error.value" class="m-0 text-z-sm text-z-error-text" role="alert">
      {{ detailRequest.error.value || editRequest.error.value }}
    </p>
    <template #footer>
      <ZButton @click="showDetail = false">
        关闭
      </ZButton>
      <ZButton v-if="detail && detail.status !== 'running'" type="primary" :loading="editRequest.loading.value" @click="rerun">
        用此配置再测
      </ZButton>
    </template>
  </ZDialog>
</template>
