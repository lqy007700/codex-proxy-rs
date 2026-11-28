<script setup lang="ts">
import type { Account } from '@/api'
import type { NewApiAffinityConfig, NewApiAffinityView, NewApiTokenIdentity } from '@/api/modules/affinity'
import { ZButton, ZDialog, ZFormItem, ZInput, ZSelect, ZSwitch } from '@codex-proxy/ui'
import { computed, reactive, ref, shallowRef, watch } from 'vue'
import { getAccounts } from '@/api'
import { getNewApiAffinityRule, saveNewApiAffinityRule } from '@/api/modules/affinity'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { useRequestState } from '@/composables/useRequestState'
import { accountIdentityLabel } from '@/utils/accountIdentity'
import { affinityTargetDisabled } from '@/utils/affinity'

const props = defineProps<{
  mode: 'default' | 'token' | 'lookup'
  identity: NewApiTokenIdentity | null
  clientKeyId: string | null
  label: string
}>()
const open = defineModel<boolean>({ required: true })
const identity = reactive<NewApiTokenIdentity>({ sourceId: '', userId: '', tokenId: '' })
const clientKeyId = ref('')
const result = shallowRef<NewApiAffinityView | null>(null)
const inherit = ref(false)
const draft = reactive({ enabled: false, requestLimit: '50', targetAccount: undefined as string | undefined })
const ruleQuery = useRequestState()
const accountsQuery = useRequestState()
const mutation = useAsyncAction()
const accounts = shallowRef<Account[]>([])
const isToken = computed(() => props.mode !== 'default')
const identityComplete = computed(() => Object.values(identity).every(value => value.trim()))
const displayedConfig = computed(() => inherit.value ? result.value?.default : null)
const target = computed(() => displayedConfig.value?.targetAccount ?? (inherit.value ? undefined : draft.targetAccount))
const accountOptions = computed(() => {
  const options = accounts.value.map(account => ({
    label: accountIdentityLabel(account),
    value: account.id,
    disabled: affinityTargetDisabled(account),
  }))
  if (target.value && !options.some(option => option.value === target.value))
    options.unshift({ label: target.value, value: target.value, disabled: false })
  return options
})
const validLimit = computed(() => /^\d+$/.test(draft.requestLimit)
  && Number(draft.requestLimit) > 0 && Number(draft.requestLimit) <= 0xFFFF_FFFF)
const canSave = computed(() => !!result.value && !ruleQuery.loading.value && !mutation.loading.value
  && (!isToken.value || identityComplete.value)
  && (inherit.value || (validLimit.value && (!draft.enabled || !!draft.targetAccount)
    && (!isToken.value || !draft.targetAccount || !!clientKeyId.value.trim()))))

function setDraft(config: NewApiAffinityConfig) {
  draft.enabled = config.enabled
  draft.requestLimit = String(config.requestLimit)
  draft.targetAccount = config.targetAccount ?? undefined
}

async function loadRule() {
  if (isToken.value && !identityComplete.value)
    return
  const requestId = ruleQuery.start()
  result.value = null
  try {
    const selected = isToken.value
      ? {
          sourceId: identity.sourceId.trim(),
          userId: identity.userId.trim(),
          tokenId: identity.tokenId.trim(),
        }
      : null
    const response = await getNewApiAffinityRule(selected, { signal: ruleQuery.signal, silent: true })
    if (!ruleQuery.isCurrent(requestId))
      return
    result.value = response
    inherit.value = isToken.value && !response.overrideConfig
    setDraft(response.overrideConfig ?? response.default)
  }
  catch (cause) { ruleQuery.fail(requestId, cause) }
  finally { ruleQuery.finish(requestId) }
}

async function loadAccounts(search = '') {
  const requestId = accountsQuery.start()
  try {
    const page = await getAccounts({
      page: 1,
      pageSize: 50,
      provider: 'openai',
      search: search.trim() || undefined,
    }, { signal: accountsQuery.signal, silent: true })
    if (accountsQuery.isCurrent(requestId))
      accounts.value = page.items.filter(account => account.schedulingRole !== 'fallback')
  }
  catch (cause) { accountsQuery.fail(requestId, cause) }
  finally { accountsQuery.finish(requestId) }
}

async function save() {
  if (!canSave.value)
    return
  await mutation.run(async () => {
    await saveNewApiAffinityRule({
      identity: isToken.value
        ? {
            sourceId: identity.sourceId.trim(),
            userId: identity.userId.trim(),
            tokenId: identity.tokenId.trim(),
          }
        : null,
      clientKeyId: isToken.value
        ? clientKeyId.value.trim() || null
        : null,
      config: inherit.value
        ? null
        : {
            enabled: draft.enabled,
            requestLimit: Number(draft.requestLimit),
            targetAccount: draft.targetAccount || null,
          },
    })
    open.value = false
  })
}

watch(() => [identity.sourceId, identity.userId, identity.tokenId], () => {
  ruleQuery.invalidate()
  ruleQuery.error.value = ''
  result.value = null
}, { flush: 'sync' })
watch(open, (value) => {
  if (!value) {
    ruleQuery.invalidate()
    accountsQuery.invalidate()
    return
  }
  Object.assign(identity, props.identity ?? { sourceId: '', userId: '', tokenId: '' })
  clientKeyId.value = props.clientKeyId ?? ''
  result.value = null
  ruleQuery.error.value = ''
  accounts.value = []
  void loadAccounts()
  if (props.mode !== 'lookup')
    void loadRule()
}, { flush: 'post' })
</script>

<template>
  <ZDialog v-model="open" :title="isToken ? 'Token 切换规则' : '全局切换规则'" width="36rem" :show-close="!mutation.loading.value" :close-on-click-modal="!mutation.loading.value" :close-on-press-escape="!mutation.loading.value">
    <template v-if="mode === 'lookup'">
      <ZFormItem label="身份来源 ID" required>
        <ZInput v-model="identity.sourceId" aria-label="身份来源 ID" :disabled="mutation.loading.value" />
      </ZFormItem>
      <ZFormItem label="New API 用户 ID" required>
        <ZInput v-model="identity.userId" aria-label="New API 用户 ID" :disabled="mutation.loading.value" />
      </ZFormItem>
      <ZFormItem label="New API Token ID" required>
        <ZInput v-model="identity.tokenId" aria-label="New API Token ID" :disabled="mutation.loading.value" />
      </ZFormItem>
      <ZFormItem label="CPR Key ID">
        <ZInput v-model="clientKeyId" aria-label="CPR Key ID" :disabled="mutation.loading.value" />
        <p class="mb-0 mt-1 text-z-xs text-z-text-secondary">
          保存目标账号时填写关联此来源的当前 CPR Key ID，用于校验账号范围
        </p>
      </ZFormItem>
      <ZButton :loading="ruleQuery.loading.value" :disabled="!identityComplete || mutation.loading.value" @click="loadRule">
        查询规则
      </ZButton>
    </template>
    <p v-else-if="isToken" class="mt-0 break-all text-z-sm text-z-text-secondary">
      {{ label }}
    </p>
    <div v-if="ruleQuery.error.value" role="alert" class="my-4 flex items-center gap-2 text-z-sm text-z-error">
      {{ ruleQuery.error.value }}<ZButton size="small" @click="loadRule">
        重试
      </ZButton>
    </div>
    <p v-else-if="ruleQuery.loading.value" class="text-z-sm text-z-text-secondary">
      正在加载规则
    </p>
    <template v-if="result">
      <ZFormItem v-if="isToken" label="继承全局规则">
        <ZSwitch v-model="inherit" aria-label="继承全局规则" :disabled="mutation.loading.value" />
      </ZFormItem>
      <ZFormItem label="启用切换规则">
        <ZSwitch :model-value="displayedConfig?.enabled ?? draft.enabled" aria-label="启用切换规则" :disabled="inherit || mutation.loading.value" @update:model-value="draft.enabled = $event" />
      </ZFormItem>
      <ZFormItem label="前 N 次请求" required>
        <ZInput :model-value="displayedConfig ? String(displayedConfig.requestLimit) : draft.requestLimit" aria-label="前 N 次请求" inputmode="numeric" :disabled="inherit || mutation.loading.value" @update:model-value="draft.requestLimit = $event" />
        <p class="mb-0 mt-1 text-z-xs text-z-text-secondary">
          前 N 次优先使用本轮主账号，从第 N+1 次起切到目标账号
        </p>
        <p v-if="!inherit && !validLimit" role="alert" class="mb-0 mt-1 text-z-xs text-z-error">
          请输入 1 至 4294967295 的整数
        </p>
      </ZFormItem>
      <ZFormItem label="切换目标账号" :required="!inherit && draft.enabled">
        <ZSelect :model-value="target" class="w-full" aria-label="切换目标账号" placeholder="搜索账号名称、邮箱或 ID" filterable remote clearable :loading="accountsQuery.loading.value" :disabled="inherit || mutation.loading.value" :options="accountOptions" @update:model-value="draft.targetAccount = $event" @query-change="loadAccounts">
          <template #option="{ option }">
            <span :title="String(option.value)">{{ option.label }}</span>
          </template>
        </ZSelect>
        <div v-if="accountsQuery.error.value" role="alert" class="mt-2 flex items-center gap-2 text-z-xs text-z-error">
          {{ accountsQuery.error.value }}<ZButton size="small" @click="loadAccounts()">
            重试
          </ZButton>
        </div>
      </ZFormItem>
      <p class="mb-0 text-z-sm text-z-text-secondary">
        规则持续保存，主绑定与请求计数随亲和有效期到期
      </p>
    </template>
    <template #footer>
      <ZButton :disabled="mutation.loading.value" @click="open = false">
        取消
      </ZButton><ZButton type="primary" :loading="mutation.loading.value" :disabled="!canSave" @click="save">
        保存
      </ZButton>
    </template>
  </ZDialog>
</template>
