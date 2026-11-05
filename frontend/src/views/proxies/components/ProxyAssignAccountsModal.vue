<script setup lang="ts">
import type { Account, OutboundProxyRecord } from '@/api'
import { defineTableColumns, ZButton, ZCheckbox, ZDialog, ZInput, ZPagination, ZTable } from '@codex-proxy/ui'
import { Search } from '@lucide/vue'
import { computed } from 'vue'
import AccountPlanBadge from '@/components/account/AccountPlanBadge.vue'
import AccountGroupMarks from '@/components/AccountGroupMarks.vue'
import ProviderIconGroup from '@/components/ProviderIconGroup.vue'
import { useProxyAccountAssignment } from '../composables/useProxyAccountAssignment'

const props = defineProps<{ proxy: OutboundProxyRecord | null }>()
const emit = defineEmits<{ assigned: [] }>()
const open = defineModel<boolean>({ required: true })
const {
  accounts,
  loading,
  error,
  search,
  pagination,
  setPage,
  setPageSize,
  selectedIds,
  selectedRowKeys,
  allSelected,
  indeterminate,
  toggleSelection,
  toggleAll,
  tooManySelected,
  maximumAccounts,
  assigning,
  assign,
} = useProxyAccountAssignment({
  isOpen: () => open.value,
  proxy: () => props.proxy,
  onAssigned: () => emit('assigned'),
})
const columns = computed(() => defineTableColumns<Account>([
  {
    key: 'selection',
    kind: 'selection',
    size: 'xs',
    align: 'center',
  },
  {
    key: 'identity',
    label: '账号',
    kind: 'identity',
    size: '2xl',
  },
  {
    key: 'provider',
    label: '平台',
    kind: 'custom',
    size: 'md',
    align: 'center',
  },
  {
    key: 'plan',
    label: '订阅',
    kind: 'custom',
    size: 'sm',
    align: 'center',
  },
  {
    key: 'groups',
    label: '分组',
    kind: 'custom',
    size: 'lg',
    align: 'center',
  },
  {
    key: 'outboundProxy',
    label: '当前代理',
    kind: 'custom',
    size: '2xl',
  },
]))
const emptyText = computed(() => error.value || (search.value.trim()
  ? '没有找到匹配的账号，请尝试其他名称或邮箱'
  : '暂无账号'))

async function confirmAssignment() {
  if (await assign())
    open.value = false
}
</script>

<template>
  <ZDialog v-model="open" title="分配账号" :description="proxy?.name" width="64rem" :show-close="!assigning" :close-on-click-modal="!assigning" :close-on-press-escape="!assigning">
    <div class="flex min-h-0 flex-col gap-3">
      <div class="flex min-w-0 items-center gap-2">
        <ZInput v-model="search" class="min-w-0 flex-1 sm:w-80 sm:flex-none" :disabled="assigning" aria-label="搜索待分配账号" placeholder="搜索账号名称或邮箱">
          <template #prefix>
            <Search class="size-4.5 text-cp-text-tertiary" />
          </template>
        </ZInput>
        <div v-if="selectedIds.size" class="ml-auto flex shrink-0 gap-2">
          <ZButton type="danger" variant="plain" :disabled="assigning" @click="selectedIds = new Set()">
            重置
          </ZButton>
          <ZButton type="primary" :loading="assigning" :disabled="!proxy?.enabled || tooManySelected" @click="confirmAssignment">
            {{ `分配 (${selectedIds.size})` }}
          </ZButton>
        </div>
      </div>
      <ZTable class="min-h-0 shrink-0 [--cp-table-row-height:56px]" :style="{ height: 'min(50dvh, 28rem)' }" :columns="columns" :data="accounts" :loading="loading" :empty-text="emptyText" :selected-row-keys="selectedRowKeys">
        <template #header-selection>
          <ZCheckbox :model-value="allSelected" :indeterminate="indeterminate" :disabled="loading || assigning" aria-label="选择当前页可分配账号" @update:model-value="toggleAll" />
        </template>
        <template #selection="{ row }">
          <ZCheckbox :model-value="selectedIds.has(row.id)" :disabled="loading || assigning || row.outboundProxyId === proxy?.id" :aria-label="`选择账号 ${row.name}`" @update:model-value="toggleSelection(row.id)" />
        </template>
        <template #identity="{ row }">
          <div class="min-w-0">
            <div class="truncate text-cp font-heavy text-cp-text" :title="row.name">
              {{ row.name }}
            </div>
            <div v-if="row.email && row.email !== row.name" class="mt-0.5 truncate font-mono text-cp-xs text-cp-text-secondary" :title="row.email">
              {{ row.email }}
            </div>
          </div>
        </template>
        <template #provider="{ row }">
          <ProviderIconGroup :provider="row.provider" :authentication-kind="row.authenticationKind" />
        </template>
        <template #plan="{ row }">
          <AccountPlanBadge :authentication-kind="row.authenticationKind" :plan-type="row.planType" :plan-type-display="row.planTypeDisplay || '—'" />
        </template>
        <template #groups="{ row }">
          <div class="flex justify-center">
            <AccountGroupMarks :groups="row.groups" />
          </div>
        </template>
        <template #outboundProxy="{ row }">
          <span v-if="row.outboundProxyId === proxy?.id" class="text-cp-success-text">已使用此代理</span>
          <span v-else class="block truncate text-cp-xs text-cp-text-secondary" :title="row.outboundProxyEndpoint ?? undefined">
            {{ row.outboundProxyEndpoint || '直连' }}
          </span>
        </template>
      </ZTable>
      <p v-if="tooManySelected" class="m-0 text-cp-sm text-cp-error-text" role="alert">
        {{ `一次最多分配 ${maximumAccounts} 个账号` }}
      </p>
      <p v-if="proxy && !proxy.enabled" class="m-0 text-cp-sm text-cp-warning-text" role="alert">
        启用此代理后可分配账号
      </p>
    </div>
    <template #footer>
      <ZPagination v-bind="pagination" class="mt-0! w-full" :disabled="loading || assigning" @current-change="setPage" @size-change="setPageSize" />
    </template>
  </ZDialog>
</template>
