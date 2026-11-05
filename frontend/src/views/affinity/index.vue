<script setup lang="ts">
import type { Account } from '@/api'
import type { AffinityFilters, AffinityOperation, AffinitySession, BindingDirective, IdentitySource } from '@/api/modules/affinity'
import { defineTableColumns, ZButton, ZCard, ZCheckbox, ZDialog, ZFormItem, ZInput, ZPageHeader, ZSegmented, ZSelect, ZSwitch, ZTable, ZTag } from '@codex-proxy/ui'
import { computed, nextTick, onMounted, reactive, ref, shallowRef, useTemplateRef, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { getAccounts } from '@/api'
import { applyAffinity, getAffinityOperations, getAffinitySessions, getIdentitySources, previewAffinity, saveIdentitySource } from '@/api/modules/affinity'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { useRequestState } from '@/composables/useRequestState'
import { accountIdentityLabel } from '@/utils/accountIdentity'
import { generateRequestId } from '@/utils/operation'
import AffinityBindingActions from './components/AffinityBindingActions.vue'

const route = useRoute()
const router = useRouter()
const view = ref('tokens')
const views = [
  { label: 'New API Key', value: 'tokens' },
  { label: '会话绑定', value: 'sessions' },
  { label: '用户', value: 'users' },
  { label: '账号', value: 'accounts' },
  { label: '操作记录', value: 'operations' },
  { label: '身份来源', value: 'sources' },
]
const filters = reactive<AffinityFilters>({})
const sessions = shallowRef<AffinitySession[]>([])
const sources = shallowRef<IdentitySource[]>([])
const operations = shallowRef<AffinityOperation[]>([])
const nextCursor = shallowRef<string | null>(null)
const cursor = shallowRef<string | undefined>()
const previousCursors = ref<(string | undefined)[]>([])
const selected = ref(new Set<string>())
const query = useRequestState()
const { loading, error } = query
const mutation = useAsyncAction()
const { loading: saving } = mutation
const operation = shallowRef<AffinityOperation | null>(null)
const showOperation = ref(false)
const operationDialog = useTemplateRef<InstanceType<typeof ZDialog>>('operationDialog')
const operationSelection = shallowRef<AffinitySession[]>([])
const action = ref<BindingDirective['action']>('target')
const targetAccount = ref<string>()
const targetAccounts = shallowRef<Account[]>([])
const accountNames = shallowRef(new Map<string, string>())
const accountNamesQuery = useRequestState()
const accountQuery = useRequestState()
const { loading: accountsLoading, error: accountsError } = accountQuery
const source = reactive<IdentitySource>({ id: '', name: '', enabled: true })
const showSource = ref(false)
const sourceNames = computed(() => new Map(sources.value.map(item => [item.id, item.name])))
const selectableSessions = computed(() => sessions.value.filter(canManage))
const selectedSessions = computed(() => selectableSessions.value.filter(session => selected.value.has(key(session))))
const statusOptions = computed(() => view.value === 'sessions'
  ? [
      { label: '已绑定', value: 'bound' },
      { label: '待调整', value: 'pending' },
    ]
  : [
      { label: '最近使用主账号', value: 'primary' },
      { label: '最近使用其他账号', value: 'spillover' },
      { label: '待调整', value: 'pending' },
    ])
const statusCounts = computed(() => ({
  primary: sessions.value.filter(item => !item.directive && item.observation.actualAccountId === item.accountId).length,
  spillover: sessions.value.filter(item => !item.directive && item.observation.actualAccountId !== item.accountId).length,
  pending: sessions.value.filter(item => item.directive).length,
}))
const allSelected = computed(() => selectableSessions.value.length > 0 && selectedSessions.value.length === selectableSessions.value.length)
const tokenOperation = computed(() => operation.value
  ? operation.value.plan.items.every(item => item.session.observation.bindingScope === 'token')
  : operationSelection.value.every(session => session.observation.bindingScope === 'token'))
const hasResults = computed(() => operation.value !== null && Object.keys(operation.value.results).length > 0)
const canApply = computed(() => {
  const current = operation.value
  return current !== null && current.plan.items.some((item) => {
    const result = current.results[key(item.session)]
    return result
      ? result.status === 'failed' || !result.auditRecorded
      : !item.rejection
  })
})
const canPreview = computed(() => operationSelection.value.length > 0 && !saving.value && (action.value !== 'target' || targetAccount.value?.trim()))
const operationTitle = computed(() => hasResults.value
  ? '账号调整结果'
  : operation.value
    ? '确认账号调整'
    : action.value === 'target'
      ? tokenOperation.value ? '设置主账号' : '指定账号'
      : action.value === 'reassign'
        ? tokenOperation.value ? '自动换一个账号' : '排除原账号重选'
        : tokenOperation.value ? '重置账号偏好' : '清除后重新选号')
const columns = computed(() => defineTableColumns<AffinitySession>([
  { key: 'selection', kind: 'selection' },
  {
    key: 'caller',
    label: view.value === 'sessions'
      ? '调用方'
      : 'New API Key',
    kind: 'identity',
    size: 'xl',
  },
  {
    key: 'session',
    label: '会话标识',
    kind: 'identity',
    size: 'xl',
  },
  {
    key: 'account',
    label: view.value === 'sessions'
      ? '会话账号 / 最近请求账号'
      : '主账号 / 最近请求账号',
    kind: 'custom',
    size: 'xl',
  },
  {
    key: 'status',
    label: '状态 / 待执行操作',
    kind: 'custom',
    size: 'xl',
  },
  {
    key: 'time',
    label: '最近准入 / 到期',
    kind: 'custom',
    size: 'xl',
  },
  {
    key: 'actions',
    label: '操作',
    kind: 'actions',
    size: 'lg',
  },
]).filter(column => view.value === 'sessions' || column.key !== 'session'))
interface Group { id: string, label: string, detail: string, sessions: AffinitySession[] }
const groups = computed<Group[]>(() => {
  const result = new Map<string, Group>()
  for (const session of sessions.value) {
    const caller = session.observation.caller
    const id = view.value === 'accounts'
      ? JSON.stringify([session.provider, session.accountId])
      : caller.mode === 'newApi'
        ? JSON.stringify([
            caller.mode,
            caller.sourceId,
            caller.userId,
          ])
        : JSON.stringify([caller.mode, session.observation.clientKeyId])
    const label = view.value === 'accounts'
      ? accountLabel(session.accountId)
      : caller.mode === 'newApi'
        ? `用户 ${caller.userId}`
        : `直连 ${session.observation.clientKeyId}`
    const detail = view.value === 'accounts'
      ? session.provider
      : caller.mode === 'newApi'
        ? sourceNames.value.get(caller.sourceId) ?? caller.sourceId
        : 'CPR Key'
    if (!result.has(id))
      result.set(id, { id, label, detail, sessions: [] })
    result.get(id)!.sessions.push(session)
  }
  return [
    ...result.values(),
  ]
})
const groupColumns = defineTableColumns<Group>([
  { key: 'selection', kind: 'selection' },
  {
    key: 'label',
    label: '身份 / 账号',
    kind: 'identity',
    size: '3xl',
  },
  {
    key: 'detail',
    label: '来源 / 渠道',
    kind: 'identity',
    size: 'xl',
  },
  { key: 'count', label: '本页 Key 数', kind: 'numeric' },
  {
    key: 'actions',
    label: '操作',
    kind: 'actions',
    size: 'lg',
  },
])
const sourceColumns = defineTableColumns<IdentitySource>([
  {
    key: 'name',
    label: '来源名称',
    kind: 'identity',
    size: 'xl',
  },
  {
    key: 'id',
    label: '稳定来源 ID',
    kind: 'mono',
    size: '3xl',
  },
  { key: 'enabled', label: '状态', kind: 'status' },
  { key: 'actions', label: '操作', kind: 'actions' },
])
const operationColumns = defineTableColumns<AffinityOperation>([
  {
    key: 'id',
    label: '操作 ID',
    kind: 'mono',
    size: '3xl',
  },
  {
    key: 'directive',
    label: '操作',
    kind: 'text',
    size: 'lg',
  },
  { key: 'count', label: '绑定数', kind: 'numeric' },
  { key: 'time', label: '预览时间', kind: 'datetime' },
  { key: 'actions', label: '结果', kind: 'actions' },
])
function canManage(session: AffinitySession) {
  return session.observation.bindingScope === 'token' || session.observation.caller.mode === 'direct'
}
function key(session: AffinitySession) {
  return `${session.provider}:${session.reference}`
}
function callerLabel(session: AffinitySession) {
  const caller = session.observation.caller
  return caller.mode === 'newApi'
    ? `用户 ${caller.userId} · Token ${caller.tokenId}`
    : `直连 Key ${session.observation.clientKeyId}`
}
function bindingContext(session: AffinitySession) {
  const caller = session.observation.caller
  const sourceName = caller.mode === 'newApi'
    ? sourceNames.value.get(caller.sourceId) ?? caller.sourceId
    : undefined
  return [
    sourceName,
    session.observation.bindingScope === 'token'
      ? undefined
      : session.reference,
  ].filter(Boolean).join(' · ')
}
function directiveLabel(directive: BindingDirective, token = false) {
  return directive.action === 'clear'
    ? token ? '重置账号偏好' : '清除后重新选号'
    : directive.action === 'reassign'
      ? token ? '自动换一个账号' : '排除原账号重选'
      : `${token ? '指定主账号' : '指定账号'} ${accountLabel(directive.accountId)}`
}
function pendingLabel(directive: BindingDirective, token = false) {
  return directive.action === 'target'
    ? `待切换至 ${accountLabel(directive.accountId)}`
    : `待执行：${directiveLabel(directive, token)}`
}
function statusLabel(session: AffinitySession) {
  if (session.directive)
    return '待调整'
  if (session.observation.bindingScope !== 'token')
    return '已绑定'
  return session.observation.actualAccountId === session.accountId
    ? '最近使用主账号'
    : '最近使用其他账号'
}
function openOperation(items: AffinitySession[], nextAction: BindingDirective['action']) {
  if (saving.value || loading.value)
    return
  // 固定本次操作对象，行内设置不改变列表的批量选择
  operationSelection.value = items.filter(canManage)
  if (!operationSelection.value.length)
    return
  operation.value = null
  action.value = nextAction
  showOperation.value = true
}
function accountLabel(id: string) {
  const target = targetAccounts.value.find(account => account.id === id)
  return accountNames.value.get(id) || (target && accountIdentityLabel(target)) || id
}
async function loadAccountNames() {
  const requestId = accountNamesQuery.start()
  const options = { signal: accountNamesQuery.signal, silent: true }
  try {
    const first = await getAccounts({ page: 1, pageSize: 200 }, options)
    if (!accountNamesQuery.isCurrent(requestId))
      return
    const names = new Map<string, string>()
    for (const account of first.items)
      names.set(account.id, accountIdentityLabel(account))
    for (let page = 2; page <= first.page.totalPages; page += 1) {
      const result = await getAccounts({ page, pageSize: first.page.pageSize }, options)
      if (!accountNamesQuery.isCurrent(requestId))
        return
      for (const account of result.items)
        names.set(account.id, accountIdentityLabel(account))
    }
    accountNames.value = names
  }
  catch {
    // 名称仅辅助展示，目录不可用时保留 ID，不阻断亲和查询或操作
  }
  finally { accountNamesQuery.finish(requestId) }
}
function refresh() {
  void loadAccountNames()
  void load()
}
function toggle(items: AffinitySession[], checked: boolean) {
  const next = new Set(selected.value)
  for (const item of items.filter(canManage)) {
    checked
      ? next.add(key(item))
      : next.delete(key(item))
  }
  selected.value = next
}
async function loadTargetAccounts(search = '') {
  const requestId = accountQuery.start()
  try {
    const providers = new Set(operationSelection.value.map(session => session.provider))
    const page = await getAccounts({
      page: 1,
      pageSize: 50,
      search: search.trim() || undefined,
      provider: providers.size === 1
        ? [...providers][0]
        : undefined,
    }, { signal: accountQuery.signal })
    if (accountQuery.isCurrent(requestId))
      targetAccounts.value = page.items
  }
  catch (cause) { accountQuery.fail(requestId, cause) }
  finally { accountQuery.finish(requestId) }
}
watch([action, operationSelection, showOperation], ([value, items, open]) => {
  // 弹窗对象决定目标范围，关闭或重开后不接受旧目录响应
  targetAccount.value = undefined
  targetAccounts.value = []
  accountQuery.invalidate()
  if (open && value === 'target' && items.length)
    void loadTargetAccounts()
})
watch([operation, saving], async ([, busy]) => {
  if (!showOperation.value || busy)
    return
  // 阶段切换或请求结束后，将失去焦点的位置收回弹窗，失败时也能继续键盘操作
  await nextTick()
  if (showOperation.value && !saving.value)
    operationDialog.value?.focus()
})
async function load() {
  const requestId = query.start()
  selected.value = new Set()
  sessions.value = []
  try {
    const [page, sourceList, history] = await Promise.all([
      view.value === 'sources' || view.value === 'operations'
        ? Promise.resolve({ items: [], nextCursor: null })
        : getAffinitySessions({
            ...Object.fromEntries(Object.entries(filters).map(([name, value]) => [name, value?.trim() || undefined])),
            bindingScope: view.value === 'sessions'
              ? 'session'
              : 'token',
            mode: view.value === 'sessions'
              ? filters.mode
              : 'newApi',
            cursor: cursor.value,
          }, { signal: query.signal }),
      view.value === 'operations'
        ? Promise.resolve(sources.value)
        : getIdentitySources({ signal: query.signal }),
      view.value === 'operations'
        ? getAffinityOperations({ signal: query.signal })
        : Promise.resolve(operations.value),
    ])
    if (!query.isCurrent(requestId))
      return
    sessions.value = page.items
    nextCursor.value = page.nextCursor
    sources.value = sourceList
    operations.value = history
  }
  catch (cause) { query.fail(requestId, cause) }
  finally { query.finish(requestId) }
}
function search() {
  cursor.value = undefined
  previousCursors.value = []
  void load()
}
function nextPage() {
  if (!nextCursor.value)
    return
  previousCursors.value.push(cursor.value)
  cursor.value = nextCursor.value
  void load()
}
function previousPage() {
  if (!previousCursors.value.length)
    return
  cursor.value = previousCursors.value.pop()
  void load()
}
function inspectCaller(session: AffinitySession) {
  const caller = session.observation.caller
  if (caller.mode !== 'newApi')
    return
  // Key 与会话的最近请求和账号可能不同，跳转只携带稳定调用方身份
  Object.keys(filters).forEach(name => delete filters[name as keyof AffinityFilters])
  filters.mode = 'newApi'
  filters.sourceId = caller.sourceId
  filters.userId = caller.userId
  filters.tokenId = caller.tokenId
  view.value = view.value === 'sessions'
    ? 'tokens'
    : 'sessions'
  search()
}
function inspect(group: Group) {
  const session = group.sessions[0]!
  if (view.value === 'accounts') {
    filters.accountId = session.accountId
  }
  else if (session.observation.caller.mode === 'newApi') {
    const caller = session.observation.caller
    filters.sourceId = caller.sourceId
    filters.userId = caller.userId
    filters.tokenId = undefined
  }
  else {
    filters.clientKeyId = session.observation.clientKeyId
  }
  view.value = 'tokens'
  search()
}
async function preview() {
  if (!canPreview.value)
    return
  await mutation.run(async () => {
    const directive: BindingDirective = action.value === 'target'
      ? {
          action: 'target',
          accountId: targetAccount.value?.trim() ?? '',
        }
      : {
          action: action.value === 'reassign'
            ? 'reassign'
            : 'clear',
        }
    operation.value = await previewAffinity({
      operationId: generateRequestId(),
      directive,
      selection: operationSelection.value.map(session => ({ provider: session.provider, reference: session.reference, revision: session.revision })),
    })
    showOperation.value = true
  })
}
async function apply() {
  if (!operation.value || !canApply.value)
    return
  await mutation.run(async () => {
    operation.value = await applyAffinity({ operationId: operation.value!.plan.operationId })
    await load()
  })
}
function editSource(item?: IdentitySource) {
  Object.assign(source, item ?? {
    id: generateRequestId(),
    name: '',
    enabled: true,
  })
  showSource.value = true
}
async function saveSource() {
  if (!source.name.trim())
    return
  await mutation.run(async () => {
    await saveIdentitySource({
      ...source,
      name: source.name.trim(),
    })
    showSource.value = false
    await load()
  })
}
function applyRoute() {
  filters.mode = route.query.mode === 'direct' || route.query.mode === 'newApi'
    ? route.query.mode
    : undefined
  view.value = route.query.bindingScope === 'session' || route.query.mode === 'direct'
    ? 'sessions'
    : 'tokens'
  for (const name of [
    'clientKeyId',
    'sourceId',
    'userId',
    'tokenId',
    'accountId',
    'requestId',
  ] as const) {
    filters[name] = typeof route.query[name] === 'string'
      ? route.query[name]
      : undefined
  }
  search()
}
watch(view, () => {
  if (view.value === 'sessions' && ['primary', 'spillover'].includes(filters.status ?? ''))
    filters.status = undefined
  search()
})
watch(() => route.query, applyRoute)
onMounted(() => {
  void loadAccountNames()
  applyRoute()
})
</script>

<template>
  <div class="w-full">
    <ZPageHeader title="账号亲和" description="管理 New API Key 的主账号，查看分流与会话归属">
      <template #actions>
        <ZButton :loading="loading" :disabled="saving" @click="refresh">
          刷新
        </ZButton>
      </template>
    </ZPageHeader>
    <ZCard class="mt-5">
      <template #header>
        <ZSegmented v-model="view" :options="views" aria-label="亲和查看方式" />
      </template>
      <template #body>
        <template v-if="view !== 'sources' && view !== 'operations'">
          <form class="grid gap-3 sm:grid-cols-3 xl:grid-cols-5" @submit.prevent="search">
            <ZSelect v-if="view === 'sessions'" v-model="filters.mode" aria-label="接入模式" placeholder="全部接入模式" clearable :options="[{ label: '直连 CPR Key', value: 'direct' }, { label: 'New API', value: 'newApi' }]" />
            <ZSelect v-model="filters.sourceId" aria-label="身份来源筛选" placeholder="全部身份来源" clearable :options="sources.map(item => ({ label: item.name, value: item.id }))" />
            <ZInput v-model="filters.clientKeyId" aria-label="CPR Key ID 筛选" placeholder="CPR Key ID" />
            <ZInput v-model="filters.userId" aria-label="用户 ID 筛选" placeholder="New API 用户 ID" />
            <ZInput v-model="filters.tokenId" aria-label="Token ID 筛选" placeholder="New API Token ID" />
            <ZInput v-model="filters.accountId" aria-label="主绑定账号筛选" placeholder="主绑定账号 ID" />
            <ZInput v-model="filters.requestId" aria-label="最近请求 ID 筛选" placeholder="最近请求 ID" />
            <ZSelect v-model="filters.status" aria-label="绑定状态" placeholder="全部绑定状态" clearable :options="statusOptions" />
            <div class="flex gap-2">
              <ZButton native-type="submit" :disabled="loading || saving">
                筛选
              </ZButton><ZButton :disabled="loading || saving" @click="Object.keys(filters).forEach(name => delete filters[name as keyof AffinityFilters]); search()">
                重置
              </ZButton>
            </div>
          </form>
          <p class="my-4 text-cp-xs text-cp-text-secondary">
            {{ view === 'sessions' ? 'New API 会话只读，改绑请操作对应 Key，直连 CPR Key 仍按会话管理' : '同一来源、用户、Token 共用主账号，临时分流保留主绑定，改绑不迁移旧会话' }}
          </p>
          <p class="-mt-2 mb-4 text-cp-xs text-cp-text-tertiary">
            仅展示已观测且未过期的绑定，每页最多 200 个索引项，用户与账号按本页 Key 汇总
          </p>
        </template>
        <div v-if="error" class="my-4 flex items-center gap-3 text-cp-error" role="alert">
          {{ error }}<ZButton @click="load">
            重试
          </ZButton>
        </div>
        <template v-else-if="view === 'sources'">
          <div class="mb-4 flex flex-wrap items-center justify-between gap-3">
            <p class="m-0 text-cp-sm text-cp-text-secondary">
              同一 New API 实例复用稳定来源，轮换 CPR Key 不改变调用方归属
            </p><ZButton type="primary" @click="editSource()">
              新增来源
            </ZButton>
          </div>
          <ZTable :columns="sourceColumns" :data="sources" :loading="loading" empty-text="暂无来源，可在此处或创建 Key 时新增">
            <template #enabled="{ row }">
              <ZTag :type="row.enabled ? 'success' : 'default'">
                {{ row.enabled ? '已启用' : '已停用' }}
              </ZTag>
            </template>
            <template #actions="{ row }">
              <ZButton size="small" @click="editSource(row)">
                编辑
              </ZButton>
            </template>
          </ZTable>
        </template>
        <ZTable v-else-if="view === 'operations'" :columns="operationColumns" :data="operations" :row-key="row => row.plan.operationId" :loading="loading" empty-text="暂无亲和操作记录">
          <template #id="{ row }">
            {{ row.plan.operationId }}
          </template><template #directive="{ row }">
            <span :title="row.plan.directive.action === 'target' ? row.plan.directive.accountId : undefined">{{ directiveLabel(row.plan.directive, row.plan.items.every(item => item.session.observation.bindingScope === 'token')) }}</span>
          </template><template #count="{ row }">
            {{ row.plan.items.length }}
          </template><template #time="{ row }">
            {{ row.plan.createdAtDisplay }}
          </template><template #actions="{ row }">
            <ZButton size="small" @click="operationSelection = []; operation = row; showOperation = true">
              查看 / 重试
            </ZButton>
          </template>
        </ZTable>
        <template v-else>
          <p v-if="view === 'tokens' || view === 'users' || view === 'accounts'" class="my-3 text-cp-sm text-cp-text-secondary" role="status">
            本页 {{ sessions.length }} 个 Key · 最近使用主账号 {{ statusCounts.primary }} · 最近使用其他账号 {{ statusCounts.spillover }} · 待调整 {{ statusCounts.pending }}
          </p>
          <div v-if="selectedSessions.length" class="my-4 flex flex-wrap items-center gap-2">
            <span class="mr-2 text-cp-sm text-cp-text-secondary">已选 {{ selectedSessions.length }} 个{{ view === 'sessions' ? '会话绑定' : ' New API Key' }}</span>
            <AffinityBindingActions batch :token="view !== 'sessions'" :disabled="saving || loading" @action="openOperation(selectedSessions, $event)" />
            <ZButton :disabled="saving" @click="selected = new Set()">
              取消选择
            </ZButton>
          </div>
          <ZTable v-if="view === 'sessions' || view === 'tokens'" :columns="columns" :data="sessions" :row-key="key" :selected-row-keys="[...selected]" :loading="loading" empty-text="当前页没有匹配的绑定，可尝试重置筛选或继续翻页">
            <template #header-selection>
              <ZCheckbox :model-value="allSelected" :indeterminate="selectedSessions.length > 0 && !allSelected" :disabled="saving || !selectableSessions.length" aria-label="选择本页可管理的绑定" @update:model-value="toggle(sessions, $event)" />
            </template>
            <template #selection="{ row }">
              <ZCheckbox v-if="canManage(row)" :model-value="selected.has(key(row))" :disabled="saving" :aria-label="`选择绑定 ${row.reference}`" @update:model-value="toggle([row], $event)" />
              <span v-else class="text-cp-xs text-cp-text-tertiary">只读</span>
            </template>
            <template #caller="{ row }">
              <div class="truncate" :title="callerLabel(row)">
                {{ view === 'tokens' && row.observation.caller.mode === 'newApi' ? `Token ${row.observation.caller.tokenId}` : callerLabel(row) }}
              </div><div class="mt-1 truncate text-cp-xs text-cp-text-tertiary">
                {{ row.observation.caller.mode === 'newApi' ? `${view === 'tokens' ? `用户 ${row.observation.caller.userId} · ` : ''}${sourceNames.get(row.observation.caller.sourceId) ?? row.observation.caller.sourceId}` : '直连 CPR' }}
              </div>
              <div class="mt-2 grid gap-1 whitespace-normal text-cp-xs sm:hidden">
                <span>{{ view === 'sessions' ? '会话' : '主账号' }}：{{ accountLabel(row.accountId) }}</span>
                <span>最近请求：{{ accountLabel(row.observation.actualAccountId) }}</span>
                <span>{{ statusLabel(row) }}</span>
                <span v-if="row.directive">{{ pendingLabel(row.directive, row.observation.bindingScope === 'token') }}</span>
              </div>
            </template>
            <template #session="{ row }">
              <span class="block truncate font-mono text-cp-xs" :title="row.reference">{{ row.reference }}</span><span class="text-cp-xs text-cp-text-tertiary">{{ row.provider }}</span>
            </template>
            <template #account="{ row }">
              <span class="block truncate" :title="row.accountId">{{ view === 'sessions' ? '会话' : '主账号' }}：{{ accountLabel(row.accountId) }}</span>
              <span class="mt-1 block truncate text-cp-xs text-cp-text-tertiary" :title="row.observation.actualAccountId">最近请求：{{ accountLabel(row.observation.actualAccountId) }}</span>
            </template>
            <template #status="{ row }">
              <ZTag :type="row.directive || (row.observation.bindingScope === 'token' && row.observation.actualAccountId !== row.accountId) ? 'warning' : 'success'">
                {{ statusLabel(row) }}
              </ZTag>
              <span v-if="row.directive" class="mt-1 block whitespace-normal text-cp-xs text-cp-text-secondary">
                {{ pendingLabel(row.directive, row.observation.bindingScope === 'token') }}
              </span>
            </template>
            <template #time="{ row }">
              <span class="block whitespace-nowrap font-mono text-cp-xs" :title="`最近准入 ${row.lastAdmissionAtDisplay}`">{{ row.lastAdmissionAtDisplay }}</span>
              <span class="mt-1 block whitespace-nowrap text-cp-xs text-cp-text-tertiary">到期 {{ row.expiresAtDisplay }}</span>
            </template>
            <template #actions="{ row }">
              <div class="flex flex-wrap gap-1">
                <AffinityBindingActions v-if="canManage(row)" :token="row.observation.bindingScope === 'token'" :caller="callerLabel(row)" :disabled="saving || loading" @action="openOperation([row], $event)" />
                <ZButton size="small" @click="router.push({ path: '/usage', query: { requestId: row.observation.requestId } })">
                  请求
                </ZButton><ZButton v-if="row.observation.caller.mode === 'newApi'" size="small" @click="inspectCaller(row)">
                  {{ view === 'sessions' ? '主绑定' : '会话' }}
                </ZButton><ZButton v-else size="small" @click="filters.clientKeyId = row.observation.clientKeyId; search()">
                  接入 Key
                </ZButton>
              </div>
            </template>
          </ZTable>
          <ZTable v-else :columns="groupColumns" :data="groups" :loading="loading" empty-text="当前页没有匹配的分组">
            <template #label="{ row }">
              <span class="block truncate" :title="view === 'accounts' ? row.sessions[0]?.accountId : row.label">{{ row.label }}</span>
            </template>
            <template #header-selection>
              <ZCheckbox :model-value="allSelected" :indeterminate="selectedSessions.length > 0 && !allSelected" :disabled="saving || !selectableSessions.length" aria-label="选择本页可管理的绑定" @update:model-value="toggle(sessions, $event)" />
            </template>
            <template #selection="{ row }">
              <ZCheckbox :model-value="row.sessions.every(session => selected.has(key(session)))" :indeterminate="row.sessions.some(session => selected.has(key(session))) && !row.sessions.every(session => selected.has(key(session)))" :disabled="saving" :aria-label="`选择本页 ${row.label} 的绑定`" @update:model-value="toggle(row.sessions, $event)" />
            </template>
            <template #count="{ row }">
              {{ row.sessions.length }}
            </template><template #actions="{ row }">
              <ZButton size="small" @click="inspect(row)">
                查看 Key
              </ZButton>
            </template>
          </ZTable>
          <div class="mt-4 flex justify-end gap-2">
            <ZButton :disabled="!previousCursors.length || loading || saving" @click="previousPage">
              上一页
            </ZButton><ZButton :disabled="!nextCursor || loading || saving" @click="nextPage">
              下一页
            </ZButton>
          </div>
        </template>
      </template>
    </ZCard>
    <ZDialog ref="operationDialog" v-model="showOperation" :title="operationTitle" width="48rem" :show-close="!saving" :close-on-click-modal="!saving" :close-on-press-escape="!saving">
      <template v-if="!operation">
        <p class="mt-0 text-cp-sm text-cp-text-secondary">
          本次调整 {{ operationSelection.length }} 个{{ tokenOperation ? ' New API Key' : '会话绑定' }}
        </p>
        <ZFormItem v-if="action === 'target'" :label="tokenOperation ? '目标主账号' : '目标账号'" required>
          <ZSelect v-model="targetAccount" class="w-full" aria-label="目标账号" :title="targetAccount" placeholder="搜索账号名称、邮箱或 ID" filterable remote :loading="accountsLoading" :disabled="saving" :options="targetAccounts.map(account => ({ label: accountLabel(account.id), value: account.id, disabled: !account.enabled || account.schedulingRole === 'fallback' || ['credential_invalid', 'account_banned'].includes(account.errorReason ?? '') }))" @query-change="loadTargetAccounts">
            <template #option="{ option }">
              <span :title="String(option.value)">{{ option.label }}</span>
            </template>
          </ZSelect>
          <div v-if="accountsError" role="alert" class="mt-2 flex items-center gap-2 text-cp-xs text-cp-error">
            {{ accountsError }}<ZButton size="small" :disabled="saving" @click="loadTargetAccounts()">
              重试
            </ZButton>
          </div>
        </ZFormItem>
        <p v-else class="text-cp-sm text-cp-text-secondary">
          {{ action === 'reassign' ? '排除当前账号，由系统重新选择可用账号' : '清除当前偏好，重新选号时仍可能选中原账号' }}
        </p>
        <ul class="m-0 grid max-h-72 list-none gap-3 overflow-auto p-0">
          <li v-for="session in operationSelection" :key="key(session)" class="rounded-cp-lg bg-cp-bg-layout p-3">
            <p class="m-0 text-cp-sm">
              {{ callerLabel(session) }}
            </p>
            <p class="my-1 break-all text-cp-xs text-cp-text-tertiary">
              {{ bindingContext(session) }}
            </p>
            <p class="mb-0 mt-1 break-all text-cp-sm text-cp-text-secondary">
              {{ accountLabel(session.accountId) }} → {{ action === 'target' ? targetAccount ? accountLabel(targetAccount) : '请选择目标账号' : action === 'reassign' ? '系统重选其他账号' : '重新选号' }}
            </p>
          </li>
        </ul>
      </template>
      <template v-if="operation">
        <p class="mt-0" :title="operation.plan.directive.action === 'target' ? operation.plan.directive.accountId : undefined">
          {{ directiveLabel(operation.plan.directive, operation.plan.items.every(item => item.session.observation.bindingScope === 'token')) }} · {{ operation.plan.items.length }} 个{{ operation.plan.items.every(item => item.session.observation.bindingScope === 'token') ? ' New API Key' : '会话绑定' }}
        </p>
        <p class="text-cp-sm text-cp-text-secondary">
          预览有效至 {{ operation.plan.expiresAtDisplay }}，执行时复核账号范围和绑定版本，冲突项不会覆盖
        </p>
        <ul class="m-0 grid max-h-96 list-none gap-3 overflow-auto p-0">
          <li v-for="item in operation.plan.items" :key="key(item.session)" class="rounded-cp-lg border border-cp-border p-3">
            <p class="m-0 text-cp-sm">
              {{ callerLabel(item.session) }}
            </p><p class="my-1 break-all text-cp-xs text-cp-text-tertiary">
              {{ bindingContext(item.session) }}
            </p><p class="my-1 break-all text-cp-sm text-cp-text-secondary">
              {{ accountLabel(item.session.accountId) }} → {{ operation.plan.directive.action === 'target' ? accountLabel(operation.plan.directive.accountId) : operation.plan.directive.action === 'reassign' ? '系统重选其他账号' : '重新选号' }}
            </p>
            <p v-if="operation.results[key(item.session)]" class="m-0 text-cp-sm" :class="operation.results[key(item.session)]!.status === 'applied' ? 'text-cp-success' : 'text-cp-error'" role="status">
              {{ operation.results[key(item.session)]!.status === 'applied' ? '操作已保存，等待生效' : operation.results[key(item.session)]!.message }}{{ operation.results[key(item.session)]!.auditRecorded ? '' : '（审计待补写，请重试）' }}
            </p>
            <p v-else class="m-0 text-cp-sm" :class="item.rejection ? 'text-cp-error' : 'text-cp-text-secondary'">
              {{ item.rejection ?? '待执行' }}
            </p>
          </li>
        </ul>
      </template>
      <p v-if="tokenOperation" class="mb-0 text-cp-sm text-cp-text-secondary">
        后续符合条件的新请求生效，旧会话不迁移，目标不可用时仍可能分流
      </p>
      <p v-else class="mb-0 text-cp-sm text-cp-text-secondary">
        已准入请求继续完成，原生续接保留其账号归属
      </p>
      <template #footer>
        <ZButton v-if="operation && !hasResults && operationSelection.length" :disabled="saving" @click="operation = null">
          返回修改
        </ZButton>
        <ZButton :disabled="saving" @click="showOperation = false">
          {{ hasResults ? '关闭' : '取消' }}
        </ZButton><ZButton v-if="!operation" type="primary" :loading="saving" :disabled="!canPreview" @click="preview">
          预览变更
        </ZButton><ZButton v-else-if="canApply" type="primary" :loading="saving" @click="apply">
          {{ hasResults ? '重试 / 补写审计' : '确认调整' }}
        </ZButton>
      </template>
    </ZDialog>
    <ZDialog v-model="showSource" title="身份来源" width="32rem" :show-close="!saving" :close-on-click-modal="!saving" :close-on-press-escape="!saving">
      <ZFormItem label="名称" required>
        <ZInput v-model="source.name" aria-label="来源名称" :disabled="saving" maxlength="128" />
      </ZFormItem><ZFormItem label="启用">
        <ZSwitch v-model="source.enabled" aria-label="启用来源" :disabled="saving" />
      </ZFormItem><p class="break-all font-mono text-cp-xs text-cp-text-tertiary">
        {{ source.id }}
      </p><p class="text-cp-sm text-cp-text-secondary">
        停用后，关联 CPR Key 的新请求将无法通过认证；重新启用后恢复
      </p>
      <template #footer>
        <ZButton :disabled="saving" @click="showSource = false">
          取消
        </ZButton><ZButton type="primary" :loading="saving" :disabled="!source.name.trim()" @click="saveSource">
          保存
        </ZButton>
      </template>
    </ZDialog>
  </div>
</template>
