<script setup lang="ts">
import type { AccountModelAccess } from '@/api'

import { ZEmpty, ZFormItem, ZIconButton, ZInput, ZScrollbar, ZTag } from '@codex-proxy/ui'

import { RefreshCw, Search, TriangleAlert } from '@lucide/vue'
import { computed, ref, watch } from 'vue'
import { getAccountModels, refreshAccountModels } from '@/api'
import { useRequestState } from '@/composables/useRequestState'
import { accountModelAccessError } from '../utils/modelAccess'
import AccountModelCleanupPopover from './AccountModelCleanupPopover.vue'

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
const inputError = ref('')
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
const selectedModels = computed(() => {
  const catalogById = new Map((catalog.value ?? []).map(item => [item.id, item]))
  return selectedModelIds.value.map(id => ({
    id,
    label: catalogById.get(id)?.label ?? id,
    unavailable: catalog.value !== undefined && !catalogById.has(id),
  }))
})
const filteredModels = computed(() => {
  const query = search.value.trim().toLowerCase()
  return (catalog.value ?? []).filter(item => `${item.id} ${item.label}`.toLowerCase().includes(query))
})
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
const selectedCount = computed(() => selectedModelIds.value.length)
const selectionLimitReached = computed(() => selectedCount.value >= MODEL_LIMIT)

function select(id: string, selected: boolean) {
  if (props.disabled || !canEdit.value)
    return
  const ids = new Set(selectedModelIds.value)
  if (selected) {
    if (ids.size >= MODEL_LIMIT && !ids.has(id)) {
      inputError.value = `最多选择 ${MODEL_LIMIT} 个模型`
      return
    }
    ids.add(id)
  }
  else {
    ids.delete(id)
  }
  const next: AccountModelAccess = {
    mode: 'allowlist',
    models: [...ids],
  }
  inputError.value = selected
    ? accountModelAccessError(next) ?? ''
    : ''
  model.value = next
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
  inputError.value = ''
  error.value = ''
  void load()
}, { immediate: true })

watch(search, () => {
  inputError.value = ''
})
</script>

<template>
  <div class="grid gap-3">
    <ZFormItem>
      <template #label>
        <span class="inline-flex flex-wrap items-end gap-x-2 gap-y-1">
          <span>模型配置</span>
          <span v-if="canEdit" class="text-cp-xs leading-none font-normal text-cp-text-tertiary tabular-nums">
            已选择 {{ selectedCount }} 个
          </span>
          <span v-if="canEdit && catalogObservedAtDisplay" class="text-cp-xs leading-none font-normal text-cp-text-tertiary tabular-nums">
            目录更新于 {{ catalogObservedAtDisplay }}
          </span>
        </span>
      </template>

      <p v-if="props.allowPreserve" class="m-0 text-cp-sm text-cp-text-secondary">
        {{ props.accountId ? '批量编辑不会修改模型配置' : '账号保存后可在编辑页面配置模型' }}
      </p>

      <template v-else>
        <div class="grid gap-2">
          <div class="flex min-w-0 items-center justify-between gap-2">
            <span class="text-cp-xs font-medium text-cp-text-secondary">已选择模型</span>
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
            class="flex max-h-28 min-h-10 flex-wrap content-start gap-1.5 overflow-y-auto rounded-cp bg-cp-fill-quaternary p-2"
            aria-label="已选择模型"
          >
            <ZTag
              v-for="item in selectedModels"
              :key="item.id"
              :type="item.unavailable ? 'warning' : 'primary'"
              size="small"
              closable
              :title="item.unavailable ? `${item.id}（当前目录未返回）` : item.id"
              :disabled="disabled"
              @close="select(item.id, false)"
            >
              <span class="max-w-48 truncate">{{ item.id }}</span>
              <TriangleAlert v-if="item.unavailable" class="ml-1 inline-block size-3 align-[-2px]" aria-hidden="true" />
            </ZTag>
          </div>
          <div v-else class="rounded-cp bg-cp-fill-quaternary px-3 py-2 text-cp-xs text-cp-text-tertiary">
            暂未选择模型，请在下方目录中选择
          </div>

          <div class="flex items-center justify-between gap-2 pt-1">
            <span class="text-cp-xs font-medium text-cp-text-secondary">全部可用模型</span>
            <span v-if="catalog" class="text-cp-xs text-cp-text-tertiary tabular-nums">
              {{ filteredModels.length }} 个
            </span>
          </div>
          <ZInput v-model="search" aria-label="搜索模型" placeholder="搜索模型 ID" :disabled="disabled || loading" :aria-invalid="Boolean(inputError)">
            <template #prefix>
              <Search class="size-4" aria-hidden="true" />
            </template>
          </ZInput>
          <p v-if="inputError" class="m-0 text-cp-xs text-cp-error-text" role="alert">
            {{ inputError }}
          </p>

          <ZScrollbar v-if="filteredModels.length" max-height="16rem" class="min-w-0 -m-1">
            <div class="flex flex-wrap gap-1.5 p-1" role="group" aria-label="全部可用模型" :aria-busy="loading || undefined">
              <button
                v-for="item in filteredModels"
                :key="item.id"
                type="button"
                class="rounded-cp border-0 bg-transparent p-0 text-left outline-none transition-opacity duration-150 hover:opacity-85 focus-visible:ring-2 focus-visible:ring-cp-control-outline focus-visible:ring-offset-2 focus-visible:ring-offset-cp-bg-container disabled:cursor-not-allowed disabled:opacity-60 motion-reduce:transition-none"
                :disabled="disabled || (!selectedIds.has(item.id) && selectionLimitReached)"
                :aria-pressed="selectedIds.has(item.id)"
                :title="item.label && item.label !== item.id ? `${item.id} · ${item.label}` : item.id"
                @click="select(item.id, !selectedIds.has(item.id))"
              >
                <ZTag size="small" :type="selectedIds.has(item.id) ? 'primary' : 'default'" :disabled="disabled || (!selectedIds.has(item.id) && selectionLimitReached)">
                  <span class="max-w-48 truncate">{{ item.id }}</span>
                </ZTag>
              </button>
            </div>
          </ZScrollbar>
          <p v-else-if="loading" class="m-0 py-4 text-center text-cp-sm text-cp-text-tertiary" role="status">
            加载模型目录中
          </p>
          <ZEmpty v-else-if="!error" :title="search ? '无匹配模型' : '暂无可用模型'" :icon="search ? Search : undefined" size="small" surface="none" />
          <ZEmpty v-if="error" title="模型目录加载失败" description="请刷新后重试" size="small" surface="none" role="alert" />
        </div>
      </template>
    </ZFormItem>
  </div>
</template>
