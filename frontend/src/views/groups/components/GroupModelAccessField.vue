<script setup lang="ts">
import type { AccountModelAccess } from '@/api'

import { ZButton, ZEmpty, ZFormItem, ZIconButton, ZInput, ZSegmented } from '@codex-proxy/ui'
import { ImagePlus, ListChecks, ListMinus, Plus, RefreshCw, Search } from '@lucide/vue'
import { computed, ref, useTemplateRef, watch } from 'vue'
import { getAccountGroupModels } from '@/api'
import { useRequestState } from '@/composables/useRequestState'
import AccountModelCleanupPopover from '@/views/accounts/components/AccountModelCleanupPopover.vue'
import AccountModelOption from '@/views/accounts/components/AccountModelOption.vue'
import { accountModelAccessError, accountModelIdError, commonImageModels } from '@/views/accounts/utils/modelAccess'

const props = withDefaults(defineProps<{
  groupId?: string | null
  disabled?: boolean
}>(), { disabled: false, groupId: null })

const model = defineModel<AccountModelAccess>({ required: true })
const refreshButton = useTemplateRef<InstanceType<typeof ZIconButton>>('refreshButton')
const catalog = ref<Array<{ id: string, label: string }>>()
const search = ref('')
const searchInput = useTemplateRef<InstanceType<typeof ZInput>>('searchInput')
const selectionError = ref('')
const request = useRequestState()
const { loading, error } = request

const modes = [
  { value: 'all', label: '默认' },
  { value: 'allowlist', label: '白名单' },
  { value: 'denylist', label: '黑名单' },
]

const mode = computed({
  get: () => model.value.mode,
  set: (value: string) => {
    selectionError.value = ''
    model.value = {
      mode: value as AccountModelAccess['mode'],
      models: value === 'all'
        ? []
        : [...model.value.models],
    }
  },
})

const restricted = computed(() => mode.value === 'allowlist' || mode.value === 'denylist')
const selectedIds = computed(() => new Set(model.value.models))
const allImageModelsSelected = computed(() => commonImageModels.every(id => selectedIds.value.has(id)))
const availableIds = ref<string[]>([])
const catalogReady = computed(() => catalog.value !== undefined && !loading.value && !error.value)
const extraIds = computed(() => {
  const ids = new Set(catalog.value?.map(item => item.id))
  return availableIds.value.filter(id => !ids.has(id))
})
const missingModels = computed(() => mode.value === 'allowlist' && catalogReady.value && catalog.value?.length
  ? extraIds.value.filter(id => selectedIds.value.has(id))
  : [])
const query = computed(() => search.value.trim().toLowerCase())
const catalogResults = computed(() => (catalog.value ?? [])
  .filter(item => `${item.id} ${item.label}`.toLowerCase().includes(query.value))
  .map(item => item.id))
const allResultsSelected = computed(() => catalogResults.value.length > 0 && catalogResults.value.every(id => selectedIds.value.has(id)))
const groups = computed(() => [
  {
    key: 'selected',
    label: catalogReady.value ? '自定义模型' : '已选择模型',
    ids: extraIds.value.filter(id => id.toLowerCase().includes(query.value)),
  },
  {
    key: 'catalog',
    label: '成员账号模型',
    ids: catalogResults.value,
  },
].filter(group => group.ids.length))
const newModelId = computed(() => groups.value.length
  ? ''
  : search.value.trim())
const inputError = computed(() => selectionError.value || (newModelId.value
  ? accountModelIdError(newModelId.value)
  : ''))
const canAdd = computed(() => Boolean(newModelId.value) && !inputError.value)

function removeMissingModels(ids: string[]) {
  if (props.disabled || model.value.mode !== 'allowlist')
    return
  const removed = new Set(ids.filter(id => missingModels.value.includes(id)))
  const next: AccountModelAccess = {
    ...model.value,
    models: model.value.models.filter(id => !removed.has(id)),
  }
  if (next.models.length) {
    availableIds.value = availableIds.value.filter(id => !removed.has(id))
    model.value = next
  }
}

function restoreCleanupFocus() {
  if (!missingModels.value.length && document.activeElement === document.body)
    refreshButton.value?.focus()
}

function select(modelIds: string[], selected: boolean) {
  if (props.disabled || !restricted.value)
    return
  const ids = new Set(selectedIds.value)
  for (const id of modelIds) {
    if (selected)
      ids.add(id)
    else ids.delete(id)
  }
  const next: AccountModelAccess = {
    ...model.value,
    models: [...ids],
  }
  selectionError.value = selected
    ? (accountModelAccessError(next) ?? '')
    : ''
  if (!selectionError.value)
    model.value = next
}

function addModels(ids: string[]) {
  select(ids, true)
  if (!selectionError.value) {
    search.value = ''
    searchInput.value?.focus()
  }
}

function handleSearchEnter(event: KeyboardEvent) {
  if (event.isComposing)
    return
  event.preventDefault()
  if (canAdd.value)
    addModels([newModelId.value])
}

async function load() {
  const id = props.groupId
  if (!id || !restricted.value)
    return
  const requestId = request.start()
  try {
    const result = await getAccountGroupModels(id, { signal: request.signal, silent: true })
    if (request.isCurrent(requestId))
      catalog.value = result.models
  }
  catch (cause) {
    request.fail(requestId, cause)
  }
  finally {
    request.finish(requestId)
  }
}

watch([
  () => props.groupId,
  () => model.value.models,
], ([groupId, ids], [previousGroupId]) => {
  const retained = groupId === previousGroupId
    ? availableIds.value
    : []
  availableIds.value = [
    ...new Set([
      ...retained,
      ...ids,
    ]),
  ]
}, { immediate: true })

watch([
  () => props.groupId,
  restricted,
], () => {
  request.invalidate()
  catalog.value = undefined
  search.value = ''
  selectionError.value = ''
  error.value = ''
  void load()
}, { immediate: true })

watch(search, () => {
  selectionError.value = ''
})
</script>

<template>
  <div class="@container grid gap-4">
    <ZFormItem label="模型限制">
      <div class="flex min-w-0 items-center justify-between gap-2">
        <ZSegmented v-model="mode" aria-label="模型限制" :options="modes" :disabled="disabled" />
        <div v-if="restricted && groupId" class="flex shrink-0 items-center gap-1">
          <AccountModelCleanupPopover
            v-if="mode === 'allowlist'"
            :models="missingModels"
            :model-count="model.models.length"
            :disabled="disabled || !missingModels.length"
            @remove="removeMissingModels"
            @closed="restoreCleanupFocus"
          />
          <ZIconButton ref="refreshButton" aria-label="刷新成员模型目录" variant="plain" :loading="loading" :disabled="disabled" @click="load()">
            <template #loading>
              <RefreshCw class="size-3.5 animate-spin motion-reduce:animate-none" />
            </template>
            <RefreshCw class="size-3.5" />
          </ZIconButton>
        </div>
      </div>
    </ZFormItem>

    <template v-if="restricted">
      <div class="flex min-w-0 items-center gap-2">
        <ZInput
          ref="searchInput"
          v-model="search"
          class="min-w-0 flex-1"
          aria-label="搜索成员账号模型"
          placeholder="搜索成员账号模型或输入完整 ID"
          :disabled="disabled"
          :aria-invalid="Boolean(inputError)"
          @keydown.enter="handleSearchEnter"
        >
          <template #prefix>
            <Search class="size-4" aria-hidden="true" />
          </template>
        </ZInput>
        <ZButton class="shrink-0" :disabled="disabled || !canAdd" @click="addModels([newModelId])">
          <Plus class="size-4" aria-hidden="true" />
          添加模型
        </ZButton>
        <ZIconButton
          v-if="mode === 'allowlist'"
          variant="plain"
          aria-label="添加常用生图模型"
          title="添加常用生图模型"
          :disabled="disabled || allImageModelsSelected"
          @click="addModels(commonImageModels)"
        >
          <ImagePlus class="size-4" aria-hidden="true" />
        </ZIconButton>
      </div>
      <p v-if="inputError" class="m-0 text-z-xs text-z-error-text" role="alert">
        {{ inputError }}
      </p>
      <div v-if="groups.length" class="grid min-w-0 gap-3" :aria-busy="loading || undefined">
        <div v-for="group in groups" :key="group.key" role="group" :aria-label="group.label">
          <div class="mb-1.5 flex items-center justify-between gap-2">
            <span class="text-z-xs font-medium text-z-text-secondary">{{ group.label }}</span>
            <ZIconButton
              v-if="group.key === 'catalog'"
              type="primary"
              variant="link"
              size="small"
              class="-my-1 size-6!"
              :aria-label="allResultsSelected ? '取消全选' : '全选'"
              :disabled="disabled || loading"
              @click="select(group.ids, !allResultsSelected)"
            >
              <ListMinus v-if="allResultsSelected" aria-hidden="true" />
              <ListChecks v-else aria-hidden="true" />
            </ZIconButton>
          </div>
          <div class="flex flex-wrap gap-1.5">
            <AccountModelOption
              v-for="id in group.ids"
              :key="id"
              :model-value="selectedIds.has(id)"
              :label="id"
              :title="id"
              :disabled="disabled"
              @update:model-value="select([id], $event)"
            />
          </div>
        </div>
      </div>
      <p v-else-if="loading" class="m-0 py-4 text-center text-z-sm text-z-text-tertiary" role="status">
        加载成员模型目录中
      </p>
      <p v-else-if="mode === 'denylist' && canAdd" class="m-0 text-z-xs text-z-text-secondary">
        点击“添加模型”将该模型加入黑名单
      </p>
      <ZEmpty v-else-if="!error && !search" :title="groupId ? '暂无成员模型，请先将有模型的账号加入分组' : '保存后将展示成员账号模型，也可直接输入完整 ID 添加'" size="small" surface="none" />
      <ZEmpty v-if="error" title="成员模型目录加载失败" description="请刷新后重试" size="small" surface="none" role="alert" />
    </template>
  </div>
</template>
