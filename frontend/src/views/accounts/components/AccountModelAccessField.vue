<script setup lang="ts">
import type { AccountModelAccess } from '@/api'

import { ZButton, ZEmpty, ZFormItem, ZIconButton, ZInput, ZScrollbar, ZTag } from '@codex-proxy/ui'

import { ListChecks, ListMinus, Plus, RefreshCw, Search } from '@lucide/vue'
import { computed, ref, useTemplateRef, watch } from 'vue'
import { getAccountModels, refreshAccountModels } from '@/api'
import { useRequestState } from '@/composables/useRequestState'
import { accountModelAccessError, accountModelIdError } from '../utils/modelAccess'
import AccountModelCleanupPopover from './AccountModelCleanupPopover.vue'
import AccountModelOption from './AccountModelOption.vue'

const props = withDefaults(defineProps<{
  accountId?: string
  disabled?: boolean
  allowPreserve?: boolean
}>(), { disabled: false, allowPreserve: false })

const emit = defineEmits<{ catalogChange: [models: string[]] }>()

const MODEL_LIMIT = 256

const model = defineModel<AccountModelAccess | undefined>({ required: true })
const catalog = ref<Array<{ id: string, label: string }>>()
const catalogObservedAtDisplay = ref<string>()
const search = ref('')
const selectionError = ref('')
const searchInput = useTemplateRef<InstanceType<typeof ZInput>>('searchInput')
const request = useRequestState()
const { loading, error } = request

const canEdit = computed(() => Boolean(props.accountId) && !props.allowPreserve)
const selectedModelIds = computed(() => {
  const access = model.value
  if (!access)
    return []
  if (access.mode === 'allowlist')
    return access.models
  if (!catalog.value) {
    return access.mode === 'all'
      ? []
      : access.models
  }
  if (access.mode === 'all')
    return catalog.value.map(item => item.id)
  const blocked = new Set(access.models)
  return catalog.value
    .map(item => item.id)
    .filter(id => !blocked.has(id))
})
const selectedIds = computed(() => new Set(selectedModelIds.value))
const selectedCount = computed(() => selectedModelIds.value.length)
const selectionLimitReached = computed(() => selectedCount.value >= MODEL_LIMIT)
const selectedModels = computed(() => {
  const catalogById = new Map((catalog.value ?? []).map(item => [item.id, item]))
  return selectedModelIds.value.map(id => ({
    id,
    label: catalogById.get(id)?.label ?? id,
    unavailable: catalog.value !== undefined && !catalogById.has(id),
  }))
})
const selectedGroups = computed(() => [
  {
    key: 'custom',
    label: catalog.value === undefined
      ? '已选择模型'
      : '自定义模型',
    models: selectedModels.value.filter(item => item.unavailable || catalog.value === undefined),
  },
  {
    key: 'catalog',
    label: '目录内模型',
    models: selectedModels.value.filter(item => !item.unavailable && catalog.value !== undefined),
  },
].filter(group => group.models.length))
const filteredModels = computed(() => {
  const query = search.value.trim().toLowerCase()
  return (catalog.value ?? []).filter(item => `${item.id} ${item.label}`.toLowerCase().includes(query))
})
const allResultsSelected = computed(() => filteredModels.value.length > 0 && filteredModels.value.every(item => selectedIds.value.has(item.id)))
const newModelId = computed(() => filteredModels.value.length || selectedIds.value.has(search.value.trim())
  ? ''
  : search.value.trim())
const inputError = computed(() => selectionError.value || (newModelId.value
  ? accountModelIdError(newModelId.value)
  : ''))
const canAdd = computed(() => Boolean(newModelId.value) && !inputError.value && !selectionLimitReached.value)
const missingModels = computed(() => model.value?.mode === 'allowlist' && catalog.value?.length && !error.value && !loading.value
  ? model.value.models.filter(id => !catalog.value!.some(item => item.id === id))
  : [])
function removeMissingModels(ids: string[]) {
  if (props.disabled || !canEdit.value || model.value?.mode !== 'allowlist')
    return
  const remove = new Set(ids.filter(id => missingModels.value.includes(id)))
  const remaining = model.value.models.filter(id => !remove.has(id))
  if (remaining.length)
    model.value = { mode: 'allowlist', models: remaining }
}
function select(modelIds: string[], selected: boolean) {
  if (props.disabled || !canEdit.value)
    return
  const ids = new Set(selectedModelIds.value)
  for (const id of modelIds) {
    if (selected)
      ids.add(id)
    else ids.delete(id)
  }
  const next: AccountModelAccess = {
    mode: 'allowlist',
    models: [...ids],
  }
  selectionError.value = selected
    ? accountModelAccessError(next) ?? ''
    : ''
  if (!selectionError.value)
    model.value = next
}

function addModel() {
  if (!canAdd.value || props.disabled || !canEdit.value)
    return
  select([newModelId.value], true)
  if (!selectionError.value) {
    search.value = ''
    searchInput.value?.focus()
  }
}

function handleSearchEnter(event: KeyboardEvent) {
  if (event.isComposing)
    return
  event.preventDefault()
  addModel()
}

async function load(refresh = false) {
  const accountId = props.accountId
  if (!accountId || !canEdit.value)
    return
  const requestId = request.start()
  try {
    const result = await (refresh ? refreshAccountModels : getAccountModels)(
      { accountId },
      { signal: request.signal, silent: true },
    )
    if (request.isCurrent(requestId)) {
      catalog.value = result.models
      emit('catalogChange', result.models.map(model => model.id))
      catalogObservedAtDisplay.value = result.observedAt ? result.observedAtDisplay : undefined
    }
  }
  catch (cause) {
    request.fail(requestId, cause)
  }
  finally {
    request.finish(requestId)
  }
}

watch([
  () => props.accountId,
  () => props.allowPreserve,
], () => {
  request.invalidate()
  catalog.value = undefined
  emit('catalogChange', [])
  catalogObservedAtDisplay.value = undefined
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
  <div class="grid gap-3">
    <ZFormItem>
      <template #label>
        <span class="inline-flex flex-wrap items-end gap-x-2 gap-y-1">
          <span>模型配置</span>
          <span v-if="canEdit" class="text-z-xs leading-none font-normal text-z-text-tertiary tabular-nums">
            已选择 {{ selectedCount }} 个
          </span>
          <span v-if="canEdit && catalogObservedAtDisplay" class="text-z-xs leading-none font-normal text-z-text-tertiary tabular-nums">
            目录更新于 {{ catalogObservedAtDisplay }}
          </span>
        </span>
      </template>

      <p v-if="props.allowPreserve" class="m-0 text-z-sm text-z-text-secondary">
        {{ props.accountId ? '批量编辑不会修改模型配置' : '账号保存后可在编辑页面配置模型' }}
      </p>

      <template v-else>
        <div class="grid gap-2">
          <div class="flex min-w-0 items-center justify-between gap-2">
            <span class="text-z-xs font-medium text-z-text-secondary">已选择模型</span>
            <AccountModelCleanupPopover v-if="missingModels.length" :models="missingModels" :model-count="selectedCount" :disabled="disabled || loading" @remove="removeMissingModels" />
            <ZIconButton
              v-if="accountId"
              aria-label="刷新模型目录"
              variant="plain"
              size="small"
              :loading="loading"
              :disabled="disabled"
              @click="load(true)"
            >
              <template #loading>
                <RefreshCw class="size-3.5 animate-spin motion-reduce:animate-none" />
              </template>
              <RefreshCw class="size-3.5" />
            </ZIconButton>
          </div>

          <div
            v-if="selectedModels.length"
            class="grid max-h-36 min-h-10 gap-2 overflow-y-auto rounded-z bg-z-fill-quaternary p-2"
            aria-label="已选择模型"
          >
            <div v-for="group in selectedGroups" :key="group.key" class="grid min-w-0 gap-1.5" role="group" :aria-label="group.label">
              <span class="text-z-xs text-z-text-secondary">{{ group.label }}</span>
              <div class="flex flex-wrap gap-1.5">
                <ZTag
                  v-for="item in group.models"
                  :key="item.id"
                  type="primary"
                  size="small"
                  closable
                  :title="item.id"
                  :disabled="disabled"
                  @close="select([item.id], false)"
                >
                  <span class="max-w-48 truncate">{{ item.id }}</span>
                </ZTag>
              </div>
            </div>
          </div>
          <div v-else class="rounded-z bg-z-fill-quaternary px-3 py-2 text-z-xs text-z-text-tertiary">
            暂未选择模型，请在下方选择或添加
          </div>

          <div class="flex items-center justify-between gap-2 pt-1">
            <span class="text-z-xs font-medium text-z-text-secondary">上游目录</span>
            <div class="flex items-center gap-2">
              <span v-if="catalog" class="text-z-xs text-z-text-tertiary tabular-nums">
                {{ filteredModels.length }} 个
              </span>
              <ZIconButton
                v-if="filteredModels.length"
                :aria-label="allResultsSelected ? '取消全选搜索结果' : '全选搜索结果'"
                variant="link"
                size="small"
                :disabled="disabled || loading"
                @click="select(filteredModels.map(item => item.id), !allResultsSelected)"
              >
                <ListMinus v-if="allResultsSelected" class="size-4" />
                <ListChecks v-else class="size-4" />
              </ZIconButton>
            </div>
          </div>
          <div class="flex min-w-0 items-center gap-2">
            <ZInput ref="searchInput" v-model="search" class="min-w-0 flex-1" aria-label="搜索或添加模型" placeholder="搜索模型或输入完整 ID" :disabled="disabled || !canEdit" :aria-invalid="Boolean(inputError)" @keydown.enter="handleSearchEnter">
              <template #prefix>
                <Search class="size-4" aria-hidden="true" />
              </template>
            </ZInput>
            <ZButton class="shrink-0" :disabled="disabled || !canEdit || !canAdd" @click="addModel">
              <Plus class="size-4" aria-hidden="true" />
              添加模型
            </ZButton>
          </div>
          <p v-if="inputError" class="m-0 text-z-xs text-z-error-text" role="alert">
            {{ inputError }}
          </p>

          <ZScrollbar v-if="filteredModels.length" max-height="16rem" class="min-w-0 -m-1">
            <div class="flex flex-wrap gap-1.5 p-1" role="group" aria-label="上游目录模型" :aria-busy="loading || undefined">
              <AccountModelOption
                v-for="item in filteredModels"
                :key="item.id"
                :model-value="selectedIds.has(item.id)"
                :label="item.id"
                :title="item.label && item.label !== item.id ? `${item.id} · ${item.label}` : item.id"
                :disabled="disabled || (!selectedIds.has(item.id) && selectionLimitReached)"
                @update:model-value="select([item.id], $event)"
              />
            </div>
          </ZScrollbar>
          <p v-else-if="loading" class="m-0 py-4 text-center text-z-sm text-z-text-tertiary" role="status">
            加载模型目录中
          </p>
          <ZEmpty v-else-if="!error && !canAdd" :title="search ? '无匹配模型' : '暂无可用模型'" :icon="search ? Search : undefined" size="small" surface="none" />
          <ZEmpty v-if="error" title="模型目录加载失败" description="请刷新后重试" size="small" surface="none" role="alert" />
        </div>
      </template>
    </ZFormItem>
  </div>
</template>
