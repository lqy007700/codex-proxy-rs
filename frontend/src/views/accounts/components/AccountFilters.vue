<script setup lang="ts">
import type { AccountGroupRef } from '@/api'

import { ZButton, ZIconButton, ZInput, ZMenuItem, ZPopover, ZSelect } from '@codex-proxy/ui'
import { Download, ListTodo, MoreHorizontal, Pencil, Search, Trash2, Upload } from '@lucide/vue'

import { computed } from 'vue'
import ProviderFilter from '@/components/ProviderFilter.vue'
import { accountStatusFilterOptions } from '../constants'

const props = defineProps<{
  hasImportTasks: boolean
  activeImportCount: number
  selectedCount: number
  batchDeleting: boolean
  exportingAccounts: boolean
  exportDisabledReason: string
  groups: AccountGroupRef[]
  groupsLoading: boolean
}>()

const emit = defineEmits<{
  deleteSelected: []
  exportSelected: []
  importTasks: []
  create: []
  editSelected: []
}>()

const search = defineModel<string>('search', { required: true })
const status = defineModel<string>('status', { required: true })
const provider = defineModel<string>('provider', { required: true })
const group = defineModel<string>('group', { required: true })
const groupOptions = computed(() => [
  { label: '全部分组', value: '' },
  { label: '未分组账号', value: 'ungrouped' },
  ...props.groups.map(group => ({
    label: group.enabled
      ? group.name
      : `${group.name}（已禁用）`,
    value: group.id,
  })),
])
</script>

<template>
  <div
    class="flex w-full flex-col gap-3 xl:flex-row xl:flex-wrap xl:items-center"
    role="group"
    aria-label="账号筛选与操作"
  >
    <div
      class="grid w-full min-w-0 grid-cols-2 items-center gap-2 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_7.75rem] xl:flex xl:w-auto xl:flex-none xl:flex-wrap xl:gap-3"
    >
      <ZInput
        v-model="search"
        aria-label="搜索账号"
        placeholder="搜索账号"
        class="col-span-2 min-w-0 sm:col-span-3 xl:w-80 xl:flex-none"
      >
        <template #prefix>
          <Search class="size-4.5 text-cp-text-tertiary" />
        </template>
      </ZInput>

      <ZSelect
        v-model="status"
        :options="accountStatusFilterOptions"
        aria-label="按账号状态筛选"
        class="w-full min-w-0 xl:w-40 xl:shrink-0"
      />

      <ZSelect
        v-model="group"
        :options="groupOptions"
        :disabled="groupsLoading"
        aria-label="按账号分组筛选"
        class="w-full min-w-0 xl:w-40 xl:shrink-0"
      />

      <ProviderFilter
        v-model="provider"
        class="col-span-2 w-full sm:col-span-1 xl:w-auto xl:shrink-0"
      />
    </div>

    <div
      class="flex w-full min-w-0 flex-wrap items-center justify-end gap-2 xl:ml-auto xl:w-auto xl:shrink-0 xl:self-end"
    >
      <ZPopover v-if="selectedCount > 0" placement="bottom-end">
        <template #reference="{ open }">
          <ZIconButton
            variant="plain"
            aria-label="更多操作"
            :pressed="open"
            :aria-expanded="open"
          >
            <MoreHorizontal class="size-4.5" aria-hidden="true" />
          </ZIconButton>
        </template>

        <template #default="{ close }">
          <div class="w-32 p-1.5">
            <ZMenuItem @click="(close(), emit('editSelected'))">
              <template #icon>
                <Pencil class="size-3.5 text-cp-text-quaternary" />
              </template>
              {{ `编辑 (${selectedCount})` }}
            </ZMenuItem>
            <ZMenuItem
              :loading="exportingAccounts"
              :disabled="Boolean(exportDisabledReason)"
              :title="exportDisabledReason || undefined"
              :aria-description="exportDisabledReason || undefined"
              @click="(close(), emit('exportSelected'))"
            >
              <template #icon>
                <Download class="size-3.5 text-cp-text-quaternary" />
              </template>
              {{ `导出 (${selectedCount})` }}
            </ZMenuItem>
            <ZMenuItem
              type="danger"
              :loading="batchDeleting"
              @click="(close(), emit('deleteSelected'))"
            >
              <template #icon>
                <Trash2 class="size-3.5" />
              </template>
              {{ `删除 (${selectedCount})` }}
            </ZMenuItem>
          </div>
        </template>
      </ZPopover>
      <ZButton v-if="hasImportTasks" class="whitespace-nowrap" @click="emit('importTasks')">
        <ListTodo class="size-4" />
        导入任务 <span v-if="activeImportCount" class="font-mono text-cp-link">{{ activeImportCount }}</span>
      </ZButton>
      <slot name="actions" />
      <ZButton type="primary" class="whitespace-nowrap" @click="emit('create')">
        <Upload class="size-4" />
        导入账号
      </ZButton>
    </div>
  </div>
</template>
