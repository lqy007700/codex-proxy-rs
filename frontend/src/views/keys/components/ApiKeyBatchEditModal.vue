<script setup lang="ts">
import type { ApiKeyLimitField } from '../composables/useApiKeyFormRules'
import type { AccountGroupRef, ApiKey, ApiKeyBatchChanges } from '@/api'
import type { ProviderRequestProfileUpdates } from '@/api/modules/settings/profiles'
import { ZButton, ZConfirmDialog, ZDialog, ZForm, ZFormItem } from '@codex-proxy/ui'
import { KeyRound } from '@lucide/vue'
import { isEqual } from 'es-toolkit'
import { computed, ref, useTemplateRef, watch } from 'vue'
import { batchUpdateApiKeys } from '@/api'
import { isValidInstallationSeed } from '@/api/modules/settings/profiles'
import AccountGroupCheckboxGrid from '@/components/AccountGroupCheckboxGrid.vue'
import ProviderRequestProfilesEditor from '@/components/client-profile/ProviderRequestProfilesEditor.vue'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { notification as ZNotification } from '@/utils/notification'
import { PROVIDER_IDS } from '@/utils/providers'
import { apiKeyFormValue } from '../composables/useApiKeyEditor'
import { apiKeyLimitFields, useApiKeyLimitRules } from '../composables/useApiKeyFormRules'
import ApiKeyInstallationSeedField from './ApiKeyInstallationSeedField.vue'
import ApiKeyLimitFields from './ApiKeyLimitFields.vue'

const props = defineProps<{
  keys: ApiKey[]
  groups: AccountGroupRef[]
  groupLoading: boolean
}>()
const emit = defineEmits<{ saved: [ids: string[]] }>()
const open = defineModel<boolean>({ required: true })
const { loading, run } = useAsyncAction()
const formRef = useTemplateRef<InstanceType<typeof ZForm>>('formElement')
const rules = useApiKeyLimitRules()
const confirmAll = ref(false)

function emptyForm() {
  return {
    groupIds: undefined as string[] | undefined,
    profiles: {} as ProviderRequestProfileUpdates,
    installationSeed: '',
    dailyLimitUsd: '',
    weeklyLimitUsd: '',
    maxConcurrency: '',
    requestsPerMinute: '',
  }
}

const form = ref(emptyForm())
const originals = computed(() => props.keys.map(apiKeyFormValue))
const initialProfiles = computed(() => {
  const result: ProviderRequestProfileUpdates = {}
  for (const id of PROVIDER_IDS) {
    const first = originals.value[0]?.providerRequestProfileOverrides[id] ?? null
    if (originals.value.every(row => isEqual(first, row.providerRequestProfileOverrides[id] ?? null)))
      result[id] = first
  }
  return result
})
const hasExistingSeed = computed(() => originals.value.some(row => row.installationSeed))
const touchedLimits = ref(new Set<ApiKeyLimitField>())

function commonLimit(field: ApiKeyLimitField) {
  const first = originals.value[0]?.[field]
  return originals.value.every(row => row[field] === first)
    ? first
    : undefined
}

const mixedLimits = computed(() => apiKeyLimitFields.filter(field => commonLimit(field) === undefined && !touchedLimits.value.has(field)))

function sameGroups(left: string[], right: string[]) {
  const ids = new Set(left)
  return ids.size === right.length && right.every(id => ids.has(id))
}

const initialGroups = computed(() => {
  const first = props.keys[0]?.groups.map(group => group.id) ?? []
  return props.keys.every(key => sameGroups(first, key.groups.map(group => group.id)))
    ? first
    : undefined
})

function updateGroups(ids: string[]) {
  form.value.groupIds = initialGroups.value && sameGroups(ids, initialGroups.value)
    ? undefined
    : ids
}

const seedError = computed(() => isValidInstallationSeed(form.value.installationSeed.trim())
  ? ''
  : '请输入有效的 UUID 格式安装身份 Seed')

const changes = computed<ApiKeyBatchChanges>(() => {
  const draft = form.value
  const patch: ApiKeyBatchChanges = {}
  if (draft.groupIds !== undefined)
    patch.groupIds = draft.groupIds
  if (Object.keys(draft.profiles).length)
    patch.providerRequestProfileOverrides = draft.profiles
  if (draft.installationSeed.trim())
    patch.installationSeed = draft.installationSeed.trim()
  for (const field of touchedLimits.value) {
    const value = draft[field].trim()
    if (value === commonLimit(field))
      continue
    if (field === 'dailyLimitUsd' || field === 'weeklyLimitUsd')
      patch[field] = value || '0'
    else
      patch[field] = Number(value)
  }
  return patch
})

const canSave = computed(() => props.keys.length > 0 && Object.keys(changes.value).length > 0 && !seedError.value)

watch(open, (visible) => {
  if (visible) {
    form.value = emptyForm()
    touchedLimits.value.clear()
    for (const field of apiKeyLimitFields)
      form.value[field] = commonLimit(field) ?? ''
  }
})

async function save() {
  if (!canSave.value || loading.value || !await formRef.value?.validate().catch(() => false))
    return
  if (form.value.groupIds?.length === 0) {
    confirmAll.value = true
    return
  }
  await applyChanges()
}

async function applyChanges() {
  confirmAll.value = false
  await run(async () => {
    const ids = props.keys.map(key => key.id)
    const result = await batchUpdateApiKeys({ ids, ...changes.value })
    open.value = false
    emit('saved', ids)
    ZNotification.success({
      message: `已更新 ${result.updated} 个密钥`,
    })
  })
}
</script>

<template>
  <ZDialog
    v-model="open"
    title="批量编辑"
    :description="`已选 ${keys.length} 个密钥`"
    width="48rem"
    type="info"
    :show-close="!loading"
    :close-on-click-modal="!loading"
    :close-on-press-escape="!loading"
  >
    <template #icon>
      <KeyRound class="text-z-text" :size="20" aria-hidden="true" />
    </template>
    <ZForm ref="formElement" :model="form" :rules="rules" scroll-to-error focus-on-error novalidate class="grid gap-6" @submit="save">
      <ZFormItem label="分组">
        <AccountGroupCheckboxGrid
          :model-value="form.groupIds ?? initialGroups ?? []"
          :groups="groups"
          :loading="groupLoading"
          :disabled="loading"
          @update:model-value="updateGroups"
        />
        <p v-if="form.groupIds?.length === 0" class="mt-2 mb-0 text-z-sm text-z-text-secondary">
          保存后也可访问未分组和以后新增的账号
        </p>
      </ZFormItem>

      <div class="grid gap-4">
        <ZFormItem label="上游身份">
          <ProviderRequestProfilesEditor
            v-model:updates="form.profiles"
            :initial-profiles="initialProfiles"
            allow-inherit
            :active="open"
            :disabled="loading"
          />
        </ZFormItem>
        <ApiKeyInstallationSeedField
          v-model="form.installationSeed"
          :disabled="loading"
          :error="seedError"
          :placeholder="hasExistingSeed ? '保持原值' : '留空不修改'"
        />
      </div>

      <ApiKeyLimitFields
        v-model:daily-limit-usd="form.dailyLimitUsd"
        v-model:weekly-limit-usd="form.weeklyLimitUsd"
        v-model:max-concurrency="form.maxConcurrency"
        v-model:requests-per-minute="form.requestsPerMinute"
        :mixed="mixedLimits"
        :disabled="loading"
        @edit="touchedLimits.add($event)"
      />
    </ZForm>

    <template #footer>
      <ZButton :disabled="loading" @click="open = false">
        取消
      </ZButton>
      <ZButton type="primary" :disabled="!canSave" :loading="loading" @click="save">
        保存
      </ZButton>
    </template>
  </ZDialog>
  <ZConfirmDialog
    v-model="confirmAll"
    title="授予全部账号权限"
    description="保存后也可访问未分组和以后新增的账号"
    confirm-text="允许访问"
    :loading="loading"
    @confirm="applyChanges"
  />
</template>
