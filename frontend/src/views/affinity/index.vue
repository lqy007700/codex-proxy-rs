<script setup lang="ts">
import type { Account } from '@/api'
import type { AffinityFilters, AffinityOperation, AffinitySession, BindingDirective, IdentitySource } from '@/api/modules/affinity'
import { defineTableColumns, ZButton, ZCard, ZCheckbox, ZDialog, ZFormItem, ZInput, ZPageHeader, ZSegmented, ZSelect, ZSwitch, ZTable, ZTag } from '@codex-proxy/ui'
import { ChevronDown } from '@lucide/vue'
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
const tabs = [
  { label: '按 K 查看', value: 'tokens' },
  { label: '按账号查看', value: 'accounts' },
]
const managementViews = [
  { label: '直连会话管理', value: 'sessions' },
  { label: '操作记录', value: 'operations' },
  { label: '身份来源', value: 'sources' },
]
const views = [
  ...tabs,
  ...managementViews,
]
const viewLabel = computed(() => [
  ...tabs,
  ...managementViews,
].find(item => item.value === view.value)?.label ?? '按 K 查看')
const filters = reactive<AffinityFilters>({})
const sessions = shallowRef<AffinitySession[]>([])
const sessionWindows = shallowRef<AffinitySession[]>([])
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
const accountCatalog = shallowRef<Account[]>([])
const accountNamesQuery = useRequestState()
const accountQuery = useRequestState()
const { loading: accountsLoading, error: accountsError } = accountQuery
const source = reactive<IdentitySource>({ id: '', name: '', enabled: true })
const showSource = ref(false)
const sourceNames = computed(() => new Map(sources.value.map(item => [item.id, item.name])))
interface AccountAffinityRow {
  account: Account
  bindings: AffinitySession[]
}
const expandedAccountIds = ref(new Set<string>())
const accountRows = computed<AccountAffinityRow[]>(() => accountCatalog.value.map(account => ({
  account,
  bindings: sessions.value.filter(session => session.accountId === account.id),
})))
function callerGroupKey(session: AffinitySession) {
  const caller = session.observation.caller
  return caller.mode === 'newApi'
    ? `newApi:${caller.sourceId}:${caller.userId}:${caller.tokenId}`
    : caller.mode === 'direct'
      ? `direct:${session.observation.clientKeyId}`
      : ''
}
const sessionStats = computed(() => {
  const stats = new Map<string, { total: number, active: number }>()
  for (const session of sessionWindows.value) {
    const group = callerGroupKey(session)
    if (!group)
      continue
    const current = stats.get(group) ?? { total: 0, active: 0 }
    current.total += 1
    if (session.active)
      current.active += 1
    stats.set(group, current)
  }
  return stats
})
function statsFor(session: AffinitySession) {
  return sessionStats.value.get(callerGroupKey(session)) ?? { total: 0, active: 0 }
}
function statsForBindings(bindings: AffinitySession[]) {
  return bindings.reduce((summary, binding) => {
    const stats = statsFor(binding)
    summary.total += stats.total
    summary.active += stats.active
    return summary
  }, { total: 0, active: 0 })
}
const selectableSessions = computed(() => sessions.value.filter(canManage))
const selectedSessions = computed(() => selectableSessions.value.filter(session => selected.value.has(key(session))))
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
      ? tokenOperation.value ? '更换主账号' : '指定账号'
      : action.value === 'reassign'
        ? tokenOperation.value ? '自动换一个账号' : '排除原账号重选'
        : tokenOperation.value ? '重置账号偏好' : '清除后重新选号')
const columns = computed(() => defineTableColumns<AffinitySession>([
  ...(view.value === 'sessions'
    ? [
        { key: 'selection', kind: 'selection' as const },
      ]
    : []),
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
    key: 'sessions',
    label: '会话窗口',
    kind: 'custom',
    size: 'md',
  },
  {
    key: 'status',
    label: '状态 / 待执行操作',
    kind: 'custom',
    size: 'xl',
  },
  {
    key: 'time',
    label: '最近准入 / 绑定到期',
    kind: 'custom',
    size: 'xl',
  },
  {
    key: 'actions',
    label: '操作',
    kind: 'actions',
    size: 'xl',
  },
]).filter(column => view.value === 'sessions' || !['session', 'status'].includes(column.key)))
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
const accountColumns = defineTableColumns<AccountAffinityRow>([
  { key: 'expander', kind: 'expander', hideable: false },
  {
    key: 'identity',
    label: '账号',
    kind: 'identity',
    size: '3xl',
  },
  {
    key: 'provider',
    label: '平台/类型',
    kind: 'meta',
    size: 'md',
  },
  {
    key: 'bindings',
    label: '当前绑定 K',
    kind: 'custom',
    size: '3xl',
  },
  {
    key: 'sessionStats',
    label: '会话窗口',
    kind: 'custom',
    size: 'lg',
  },
  {
    key: 'status',
    label: '状态',
    kind: 'custom',
    size: 'lg',
  },
])
function canManage(session: AffinitySession) {
  return session.observation.bindingScope === 'token' || session.observation.caller.mode === 'direct'
}
function key(session: AffinitySession) {
  return `${session.provider}:${session.reference}`
}
function tokenLabel(session: AffinitySession) {
  const caller = session.observation.caller
  const name = session.tokenName ?? operationSelection.value.find(item => key(item) === key(session))?.tokenName
  return name || (caller.mode === 'newApi'
    ? `Token #${caller.tokenId}`
    : session.observation.clientKeyId)
}
function callerLabel(session: AffinitySession) {
  const caller = session.observation.caller
  return caller.mode === 'newApi'
    ? `${tokenLabel(session)} · 用户 #${caller.userId}`
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
    const accounts = [...first.items]
    const names = new Map<string, string>()
    for (const account of first.items)
      names.set(account.id, accountIdentityLabel(account))
    for (let page = 2; page <= first.page.totalPages; page += 1) {
      const result = await getAccounts({ page, pageSize: first.page.pageSize }, options)
      if (!accountNamesQuery.isCurrent(requestId))
        return
      for (const account of result.items)
        names.set(account.id, accountIdentityLabel(account))
      accounts.push(...result.items)
    }
    accountNames.value = names
    accountCatalog.value = accounts
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
function toggleAccount(accountId: string) {
  const next = new Set(expandedAccountIds.value)
  if (next.has(accountId))
    next.delete(accountId)
  else
    next.add(accountId)
  expandedAccountIds.value = next
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
    const [page, sourceList, history, windows] = await Promise.all([
      view.value === 'sources' || view.value === 'operations'
        ? Promise.resolve({ items: [], nextCursor: null })
        : view.value === 'accounts'
          ? loadAllTokenAffinity()
          : getAffinitySessions({
              ...Object.fromEntries(Object.entries(filters).map(([name, value]) => [name, value?.trim() || undefined])),
              bindingScope: view.value === 'sessions'
                ? 'session'
                : 'token',
              mode: view.value === 'sessions'
                ? filters.mode ?? 'direct'
                : 'newApi',
              cursor: cursor.value,
            }, { signal: query.signal }),
      view.value === 'operations'
        ? Promise.resolve(sources.value)
        : getIdentitySources({ signal: query.signal }),
      view.value === 'operations'
        ? getAffinityOperations({ signal: query.signal })
        : Promise.resolve(operations.value),
      view.value === 'tokens' || view.value === 'accounts'
        ? loadAllSessionAffinity()
        : Promise.resolve([] as AffinitySession[]),
    ])
    if (!query.isCurrent(requestId))
      return
    sessions.value = page.items
    nextCursor.value = page.nextCursor
    sources.value = sourceList
    operations.value = history
    sessionWindows.value = windows
  }
  catch (cause) { query.fail(requestId, cause) }
  finally { query.finish(requestId) }
}

async function loadAllSessionAffinity() {
  const items: AffinitySession[] = []
  let nextCursor: string | undefined
  do {
    const page = await getAffinitySessions({
      bindingScope: 'session',
      mode: 'newApi',
      cursor: nextCursor,
    }, { signal: query.signal })
    items.push(...page.items)
    nextCursor = page.nextCursor ?? undefined
  } while (nextCursor)
  return items
}
async function loadAllTokenAffinity() {
  const items: AffinitySession[] = []
  let nextCursor: string | undefined
  do {
    const page = await getAffinitySessions({
      bindingScope: 'token',
      mode: 'newApi',
      cursor: nextCursor,
    }, { signal: query.signal })
    items.push(...page.items)
    nextCursor = page.nextCursor ?? undefined
  } while (nextCursor)
  return { items, nextCursor: null }
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
function viewRequests(session: AffinitySession) {
  const caller = session.observation.caller
  void router.push({
    path: '/usage',
    query: caller.mode === 'newApi'
      ? {
          callerMode: caller.mode,
          callerSourceId: caller.sourceId,
          callerUserId: caller.userId,
          callerTokenId: caller.tokenId,
        }
      : { requestId: session.observation.requestId },
  })
}
function selectView(value: string) {
  if (saving.value || loading.value)
    return
  // 管理入口显式切换范围，保留路由深链的筛选直到用户主动返回全部
  void router.replace({
    path: '/affinity',
    query: value === 'accounts'
      ? { view: 'accounts' }
      : value === 'sessions'
        ? { mode: 'direct' }
        : value === 'tokens'
          ? {}
          : { management: value },
  })
}
function clearFilters() {
  void router.replace({
    path: '/affinity',
    query: view.value === 'accounts'
      ? { view: 'accounts' }
      : view.value === 'sessions'
        ? { mode: 'direct' }
        : {},
  })
}
const filterContext = computed(() => Object.entries(filters)
  .filter(([name, value]) => value && name !== 'mode')
  .map(([name, value]) => `${({
    sourceId: '来源',
    clientKeyId: 'CPR Key',
    userId: '用户',
    tokenId: 'Token',
    accountId: '账号',
    requestId: '请求',
  } as Record<string, string>)[name] ?? name} ${value}`)
  .join(' · '))
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
  if (route.query.view === 'accounts') {
    view.value = 'accounts'
  }
  else if (route.query.bindingScope === 'session' || route.query.mode === 'direct') {
    view.value = 'sessions'
  }
  else {
    view.value = route.query.management === 'sources' || route.query.management === 'operations'
      ? route.query.management
      : 'tokens'
  }
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
watch(() => route.query, applyRoute)
onMounted(() => {
  void loadAccountNames()
  applyRoute()
})

function bindingStatusLabel(session: AffinitySession) {
  if (session.directive)
    return pendingLabel(session.directive, true)
  return session.accountId === session.observation.actualAccountId
    ? '主账号'
    : '最近请求使用其他账号'
}

function openTokenView(session: AffinitySession) {
  const caller = session.observation.caller
  void router.replace({
    path: '/affinity',
    query: caller.mode === 'newApi'
      ? {
          sourceId: caller.sourceId,
          userId: caller.userId,
          tokenId: caller.tokenId,
        }
      : { accountId: session.accountId },
  })
}
</script>

<template>
  <div class="w-full">
    <ZPageHeader title="账号亲和" description="管理 New API Key 与 CPR 主账号的绑定">
      <template #actions>
        <ZButton :loading="loading" :disabled="saving" @click="refresh">
          刷新
        </ZButton>
      </template>
    </ZPageHeader>
    <ZCard class="mt-5">
      <template #header>
        <div class="flex flex-wrap items-center justify-between gap-3">
          <span class="text-cp-sm font-medium">{{ viewLabel }}</span>
          <ZSegmented v-model="view" aria-label="账号亲和视图" :options="views" :disabled="saving || loading" @update:model-value="selectView" />
        </div>
      </template>
      <template #body>
        <div v-if="filterContext && view !== 'sources' && view !== 'operations' && view !== 'accounts'" class="mb-4 flex flex-wrap items-center gap-2 text-cp-xs text-cp-text-secondary">
          <span class="break-all">当前范围：{{ filterContext }}</span>
          <ZButton size="small" :disabled="saving || loading" @click="clearFilters">
            {{ view === 'sessions' ? '查看全部直连绑定' : '查看全部 Key' }}
          </ZButton>
        </div>
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
        <template v-else-if="view === 'accounts'">
          <p class="mb-4 text-cp-sm text-cp-text-secondary">
            按账号查看当前绑定的 New API Key，仅展示已观测且未过期的主绑定
          </p>
          <ZTable :columns="accountColumns" :data="accountRows" :row-key="row => row.account.id" :expanded-row-keys="[...expandedAccountIds]" :loading="loading" empty-text="暂无账号数据">
            <template #expander="{ row }">
              <button type="button" class="inline-flex size-6 cursor-pointer items-center justify-center rounded-md border-0 bg-transparent text-cp-text-secondary transition hover:bg-cp-bg-text-hover hover:text-cp-text" :title="expandedAccountIds.has(row.account.id) ? '收起绑定 K' : '展开绑定 K'" @click.stop="toggleAccount(row.account.id)">
                <ChevronDown class="size-3.5 transition-transform" :class="expandedAccountIds.has(row.account.id) ? '' : '-rotate-90'" />
              </button>
            </template>
            <template #identity="{ row }">
              <div class="truncate" :title="accountIdentityLabel(row.account)">
                {{ accountIdentityLabel(row.account) }}
              </div>
              <div class="mt-1 truncate text-cp-xs text-cp-text-tertiary">
                {{ row.account.email || row.account.id }}
              </div>
            </template>
            <template #provider="{ row }">
              <span>{{ row.account.provider }}</span>
              <span class="mt-1 block text-cp-xs text-cp-text-tertiary">{{ row.account.authenticationKind }}</span>
            </template>
            <template #bindings="{ row }">
              <div v-if="row.bindings.length" class="flex flex-wrap gap-1">
                <ZTag v-for="binding in row.bindings.slice(0, 3)" :key="key(binding)" type="info">
                  {{ tokenLabel(binding) }}
                </ZTag>
                <ZTag v-if="row.bindings.length > 3" type="default">
                  +{{ row.bindings.length - 3 }}
                </ZTag>
              </div>
              <span v-else class="text-cp-xs text-cp-text-tertiary">暂无已观测绑定</span>
            </template>
            <template #sessionStats="{ row }">
              <template v-if="statsForBindings(row.bindings).total">
                <span>{{ statsForBindings(row.bindings).total }} 个窗口</span>
                <span class="mt-1 block text-cp-xs text-cp-text-tertiary">进行中 {{ statsForBindings(row.bindings).active }}</span>
              </template>
              <span v-else class="text-cp-xs text-cp-text-tertiary">暂无会话</span>
            </template>
            <template #status="{ row }">
              <ZTag :type="row.bindings.length ? 'success' : 'default'">
                {{ row.bindings.length ? `${row.bindings.length} 个 K` : '未绑定' }}
              </ZTag>
            </template>
            <template #expanded="{ row }">
              <div class="grid gap-2 p-4">
                <div v-if="!row.bindings.length" class="text-cp-sm text-cp-text-tertiary">
                  当前账号没有已观测且未过期的 K 绑定
                </div>
                <div v-for="binding in row.bindings" :key="key(binding)" class="grid gap-2 rounded-cp bg-cp-fill-quaternary p-3 md:grid-cols-[minmax(0,1.4fr)_minmax(0,1fr)_minmax(0,1fr)_minmax(0,0.8fr)_auto] md:items-center">
                  <div class="min-w-0">
                    <div class="truncate text-cp-sm" :title="tokenLabel(binding)">
                      {{ tokenLabel(binding) }}
                    </div>
                    <div class="mt-1 truncate text-cp-xs text-cp-text-tertiary">
                      Token #{{ binding.observation.caller.mode === 'newApi' ? binding.observation.caller.tokenId : '-' }} · 用户 #{{ binding.observation.caller.mode === 'newApi' ? binding.observation.caller.userId : '-' }}
                    </div>
                  </div>
                  <div class="text-cp-xs text-cp-text-secondary">
                    {{ binding.provider }}
                  </div>
                  <div>
                    <ZTag :type="binding.directive || binding.accountId !== binding.observation.actualAccountId ? 'warning' : 'success'">
                      {{ bindingStatusLabel(binding) }}
                    </ZTag>
                    <div class="mt-1 text-cp-xs text-cp-text-tertiary">
                      到期 {{ binding.expiresAtDisplay }}
                    </div>
                  </div>
                  <div class="text-cp-xs text-cp-text-secondary">
                    <span>{{ statsFor(binding).total }} 个窗口</span>
                    <span class="mt-1 block text-cp-xs text-cp-text-tertiary">进行中 {{ statsFor(binding).active }}</span>
                  </div>
                  <ZButton size="small" @click="openTokenView(binding)">
                    查看 K
                  </ZButton>
                </div>
              </div>
            </template>
          </ZTable>
        </template>
        <template v-else>
          <div v-if="view === 'sessions' && selectedSessions.length" class="my-4 flex flex-wrap items-center gap-2">
            <span class="mr-2 text-cp-sm text-cp-text-secondary">已选 {{ selectedSessions.length }} 个{{ view === 'sessions' ? '会话绑定' : ' New API Key' }}</span>
            <AffinityBindingActions batch :token="view !== 'sessions'" :disabled="saving || loading" @action="openOperation(selectedSessions, $event)" />
            <ZButton :disabled="saving" @click="selected = new Set()">
              取消选择
            </ZButton>
          </div>
          <ZTable :columns="columns" :data="sessions" :row-key="key" :selected-row-keys="[...selected]" :loading="loading" empty-text="当前页没有已观测且未过期的绑定">
            <template #header-selection>
              <ZCheckbox :model-value="allSelected" :indeterminate="selectedSessions.length > 0 && !allSelected" :disabled="saving || !selectableSessions.length" aria-label="选择本页可管理的绑定" @update:model-value="toggle(sessions, $event)" />
            </template>
            <template #selection="{ row }">
              <ZCheckbox v-if="canManage(row)" :model-value="selected.has(key(row))" :disabled="saving" :aria-label="`选择绑定 ${row.reference}`" @update:model-value="toggle([row], $event)" />
              <span v-else class="text-cp-xs text-cp-text-tertiary">只读</span>
            </template>
            <template #caller="{ row }">
              <div class="truncate" :title="callerLabel(row)">
                {{ row.observation.caller.mode === 'newApi' ? tokenLabel(row) : callerLabel(row) }}
              </div><div class="mt-1 truncate text-cp-xs text-cp-text-tertiary">
                {{ row.observation.caller.mode === 'newApi' ? `Token #${row.observation.caller.tokenId} · 用户 #${row.observation.caller.userId}` : '直连 CPR' }}
              </div>
              <div v-if="row.observation.caller.mode === 'newApi'" class="mt-1 truncate text-cp-xs text-cp-text-tertiary" :title="sourceNames.get(row.observation.caller.sourceId) ?? row.observation.caller.sourceId">
                {{ sourceNames.get(row.observation.caller.sourceId) ?? row.observation.caller.sourceId }}
              </div>
              <div class="mt-2 grid gap-1 whitespace-normal text-cp-xs sm:hidden">
                <span>{{ view === 'sessions' ? '会话' : '主账号' }}：{{ accountLabel(row.accountId) }}</span>
                <span>最近请求：{{ accountLabel(row.observation.actualAccountId) }}</span>
                <span>{{ statusLabel(row) }}</span>
                <span v-if="row.directive">{{ pendingLabel(row.directive, row.observation.bindingScope === 'token') }}</span>
                <span class="min-w-0 break-words">最近准入：{{ row.lastAdmissionAtDisplay }}</span>
                <span class="min-w-0 break-words">绑定到期：{{ row.expiresAtDisplay }}</span>
              </div>
            </template>
            <template #session="{ row }">
              <span class="block truncate font-mono text-cp-xs" :title="row.reference">{{ row.reference }}</span><span class="text-cp-xs text-cp-text-tertiary">{{ row.provider }}</span>
            </template>
            <template #account="{ row }">
              <span class="block truncate" :title="row.accountId">{{ view === 'sessions' ? '会话' : '主账号' }}：{{ accountLabel(row.accountId) }}</span>
              <span class="mt-1 block truncate text-cp-xs text-cp-text-tertiary" :title="row.observation.actualAccountId">最近请求：{{ accountLabel(row.observation.actualAccountId) }}</span>
              <div v-if="view === 'tokens' && (row.directive || row.accountId !== row.observation.actualAccountId)" class="mt-2 text-cp-xs">
                <ZTag type="warning">
                  {{ row.directive ? pendingLabel(row.directive, true) : '最近使用其他账号' }}
                </ZTag>
              </div>
            </template>
            <template #sessions="{ row }">
              <span>{{ statsFor(row).total }} 个窗口</span>
              <span class="mt-1 block text-cp-xs text-cp-text-tertiary">进行中 {{ statsFor(row).active }}</span>
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
                <ZButton v-if="view === 'tokens'" size="small" :disabled="saving || loading" :aria-label="`更换 ${callerLabel(row)} 的主账号`" @click="openOperation([row], 'target')">
                  更换主账号
                </ZButton>
                <AffinityBindingActions v-else-if="canManage(row)" :token="false" :caller="callerLabel(row)" :disabled="saving || loading" @action="openOperation([row], $event)" />
                <ZButton size="small" @click="viewRequests(row)">
                  查看请求
                </ZButton>
              </div>
            </template>
          </ZTable>
          <div class="mt-4 flex flex-wrap items-center justify-between gap-2">
            <span class="text-cp-xs text-cp-text-tertiary">仅展示已观测且未过期的绑定</span>
            <div class="flex gap-2">
              <ZButton :disabled="!previousCursors.length || loading || saving" @click="previousPage">
                上一页
              </ZButton><ZButton :disabled="!nextCursor || loading || saving" @click="nextPage">
                下一页
              </ZButton>
            </div>
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
