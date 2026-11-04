<script setup lang="ts">
import type { Account } from '@/api'
import type { AffinityFilters, AffinityOperation, AffinitySession, BindingDirective, IdentitySource } from '@/api/modules/affinity'
import { defineTableColumns, ZButton, ZCard, ZCheckbox, ZDialog, ZFormItem, ZInput, ZPageHeader, ZSegmented, ZSelect, ZSwitch, ZTable, ZTag } from '@codex-proxy/ui'
import { computed, onMounted, reactive, ref, shallowRef, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { getAccounts } from '@/api'
import { applyAffinity, getAffinityOperations, getAffinitySessions, getIdentitySources, previewAffinity, saveIdentitySource } from '@/api/modules/affinity'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { useRequestState } from '@/composables/useRequestState'
import { accountIdentityLabel } from '@/utils/accountIdentity'
import { generateRequestId } from '@/utils/operation'

const route = useRoute()
const router = useRouter()
const view = ref('sessions')
const views = [
  { label: '会话', value: 'sessions' },
  { label: '用户', value: 'users' },
  { label: 'Token', value: 'tokens' },
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
const action = ref('clear')
const targetAccount = ref<string>()
const targetAccounts = shallowRef<Account[]>([])
const accountNames = shallowRef(new Map<string, string>())
const accountNamesQuery = useRequestState()
const accountQuery = useRequestState()
const { loading: accountsLoading, error: accountsError } = accountQuery
const source = reactive<IdentitySource>({ id: '', name: '', enabled: true })
const showSource = ref(false)
const sourceNames = computed(() => new Map(sources.value.map(item => [item.id, item.name])))
const selectedSessions = computed(() => sessions.value.filter(session => selected.value.has(key(session))))
const allSelected = computed(() => sessions.value.length > 0 && selectedSessions.value.length === sessions.value.length)
const canPreview = computed(() => selectedSessions.value.length > 0 && !loading.value && !saving.value && (action.value !== 'target' || targetAccount.value?.trim()))
const columns = defineTableColumns<AffinitySession>([
  { key: 'selection', kind: 'selection' },
  {
    key: 'caller',
    label: '调用方',
    kind: 'identity',
    size: '2xl',
  },
  {
    key: 'session',
    label: '会话',
    kind: 'identity',
    size: 'xl',
  },
  {
    key: 'account',
    label: '主绑定 / 最近请求账号',
    kind: 'custom',
    size: '2xl',
  },
  { key: 'status', label: '状态', kind: 'status' },
  { key: 'lastAdmissionAtDisplay', label: '最近准入', kind: 'datetime' },
  { key: 'expiresAtDisplay', label: '绑定到期', kind: 'datetime' },
  {
    key: 'actions',
    label: '操作',
    kind: 'actions',
    size: 'lg',
  },
])
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
            view.value === 'tokens'
              ? caller.tokenId
              : null,
          ])
        : JSON.stringify([caller.mode, session.observation.clientKeyId])
    const label = view.value === 'accounts'
      ? accountLabel(session.accountId)
      : caller.mode === 'newApi'
        ? `用户 ${caller.userId}${view.value === 'tokens'
          ? ` · Token ${caller.tokenId}`
          : ''}`
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
  { key: 'count', label: '本页会话数', kind: 'numeric' },
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
  { key: 'count', label: '会话数', kind: 'numeric' },
  { key: 'time', label: '预览时间', kind: 'datetime' },
  { key: 'actions', label: '结果', kind: 'actions' },
])
function key(session: AffinitySession) {
  return `${session.provider}:${session.reference}`
}
function callerLabel(session: AffinitySession) {
  const caller = session.observation.caller
  return caller.mode === 'newApi'
    ? `用户 ${caller.userId} · Token ${caller.tokenId}`
    : `直连 Key ${session.observation.clientKeyId}`
}
function directiveLabel(directive: BindingDirective) {
  return directive.action === 'clear'
    ? '清除后重新选号'
    : directive.action === 'reassign'
      ? '排除原账号重选'
      : `指定账号 ${accountLabel(directive.accountId)}`
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
  for (const item of items) {
    checked
      ? next.add(key(item))
      : next.delete(key(item))
  }
  selected.value = next
}
async function loadTargetAccounts(search = '') {
  const requestId = accountQuery.start()
  try {
    const providers = new Set(selectedSessions.value.map(session => session.provider))
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
watch(action, (value) => {
  if (value === 'target')
    void loadTargetAccounts()
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
function inspect(group: Group) {
  const session = group.sessions[0]!
  if (view.value === 'accounts') {
    filters.accountId = session.accountId
  }
  else if (session.observation.caller.mode === 'newApi') {
    const caller = session.observation.caller
    filters.sourceId = caller.sourceId
    filters.userId = caller.userId
    filters.tokenId = view.value === 'tokens'
      ? caller.tokenId
      : undefined
  }
  else {
    filters.clientKeyId = session.observation.clientKeyId
  }
  view.value = 'sessions'
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
      selection: selectedSessions.value.map(session => ({ provider: session.provider, reference: session.reference, revision: session.revision })),
    })
    showOperation.value = true
  })
}
async function apply() {
  if (!operation.value)
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
  void load()
})
watch(() => route.query, applyRoute)
onMounted(() => {
  void loadAccountNames()
  applyRoute()
})
</script>

<template>
  <div class="w-full">
    <ZPageHeader title="账号亲和" description="按调用方管理会话绑定，查看实际请求账号与调度结果">
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
            <ZSelect v-model="filters.mode" aria-label="接入模式" placeholder="全部接入模式" clearable :options="[{ label: '直连 CPR Key', value: 'direct' }, { label: 'New API', value: 'newApi' }]" />
            <ZSelect v-model="filters.sourceId" aria-label="身份来源筛选" placeholder="全部身份来源" clearable :options="sources.map(item => ({ label: item.name, value: item.id }))" />
            <ZInput v-model="filters.clientKeyId" aria-label="CPR Key ID 筛选" placeholder="CPR Key ID" />
            <ZInput v-model="filters.userId" aria-label="用户 ID 筛选" placeholder="New API 用户 ID" />
            <ZInput v-model="filters.tokenId" aria-label="Token ID 筛选" placeholder="New API Token ID" />
            <ZInput v-model="filters.accountId" aria-label="主绑定账号筛选" placeholder="主绑定账号 ID" />
            <ZInput v-model="filters.requestId" aria-label="最近请求 ID 筛选" placeholder="最近请求 ID" />
            <ZSelect v-model="filters.status" aria-label="绑定状态" placeholder="全部绑定状态" clearable :options="[{ label: '已绑定', value: 'bound' }, { label: '待重选', value: 'pending' }]" />
            <div class="flex gap-2">
              <ZButton native-type="submit" :disabled="loading || saving">
                筛选
              </ZButton><ZButton :disabled="loading || saving" @click="Object.keys(filters).forEach(name => delete filters[name as keyof AffinityFilters]); search()">
                重置
              </ZButton>
            </div>
          </form>
          <p class="my-4 text-cp-xs text-cp-text-secondary">
            仅展示已观测且未过期的会话，按最多 200 个索引项翻页；用户、Token 与账号按本页汇总。无会话标识的请求不建立绑定
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
            <span :title="row.plan.directive.action === 'target' ? row.plan.directive.accountId : undefined">{{ directiveLabel(row.plan.directive) }}</span>
          </template><template #count="{ row }">
            {{ row.plan.items.length }}
          </template><template #time="{ row }">
            {{ row.plan.createdAtDisplay }}
          </template><template #actions="{ row }">
            <ZButton size="small" @click="operation = row; showOperation = true">
              查看 / 重试
            </ZButton>
          </template>
        </ZTable>
        <template v-else>
          <div class="my-4 flex flex-wrap items-center gap-2">
            <span class="mr-2 text-cp-sm text-cp-text-secondary">已选 {{ selectedSessions.length }} 个本页会话</span>
            <ZSelect v-model="action" class="w-48" aria-label="亲和操作" :disabled="saving" :options="[{ label: '清除后重新选号', value: 'clear' }, { label: '排除原账号重选', value: 'reassign' }, { label: '指定目标账号', value: 'target' }]" />
            <ZSelect v-if="action === 'target'" v-model="targetAccount" class="sm:w-72" aria-label="目标账号" :title="targetAccount" placeholder="搜索账号名称、邮箱或 ID" filterable remote :loading="accountsLoading" :disabled="saving" :options="targetAccounts.map(account => ({ label: accountLabel(account.id), value: account.id, disabled: !account.enabled || account.schedulingRole === 'fallback' }))" @query-change="loadTargetAccounts">
              <template #option="{ option }">
                <span :title="String(option.value)">{{ option.label }}</span>
              </template>
            </ZSelect>
            <span v-if="action === 'target' && accountsError" role="alert" class="text-cp-xs text-cp-error">{{ accountsError }}<ZButton size="small" @click="loadTargetAccounts()">重试</ZButton></span>
            <ZButton type="primary" :disabled="!canPreview" :loading="saving" @click="preview">
              预览操作
            </ZButton>
          </div>
          <ZTable v-if="view === 'sessions'" :columns="columns" :data="sessions" :row-key="key" :selected-row-keys="[...selected]" :loading="loading" empty-text="当前页没有匹配的会话；如可翻页，请继续查看">
            <template #header-selection>
              <ZCheckbox :model-value="allSelected" :indeterminate="selectedSessions.length > 0 && !allSelected" :disabled="saving" aria-label="选择本页全部会话" @update:model-value="toggle(sessions, $event)" />
            </template>
            <template #selection="{ row }">
              <ZCheckbox :model-value="selected.has(key(row))" :disabled="saving" :aria-label="`选择会话 ${row.reference}`" @update:model-value="toggle([row], $event)" />
            </template>
            <template #caller="{ row }">
              <div class="truncate" :title="callerLabel(row)">
                {{ callerLabel(row) }}
              </div><div class="mt-1 truncate text-cp-xs text-cp-text-tertiary">
                {{ row.observation.caller.mode === 'newApi' ? sourceNames.get(row.observation.caller.sourceId) ?? row.observation.caller.sourceId : '直连 CPR' }}
              </div>
            </template>
            <template #session="{ row }">
              <span class="block truncate font-mono text-cp-xs" :title="row.reference">{{ row.reference }}</span><span class="text-cp-xs text-cp-text-tertiary">{{ row.provider }}</span>
            </template>
            <template #account="{ row }">
              <span class="block truncate" :title="row.accountId">{{ accountLabel(row.accountId) }}</span><span class="mt-1 block truncate text-cp-xs text-cp-text-tertiary" :title="row.observation.actualAccountId">{{ accountLabel(row.observation.actualAccountId) }}</span>
            </template>
            <template #status="{ row }">
              <ZTag :type="row.directive ? 'warning' : 'success'" :title="row.directive ? directiveLabel(row.directive) : undefined">
                {{ row.directive ? '待重选' : '已绑定' }}
              </ZTag>
            </template>
            <template #actions="{ row }">
              <div class="flex gap-1">
                <ZButton size="small" @click="router.push({ path: '/usage', query: { requestId: row.observation.requestId } })">
                  请求
                </ZButton><ZButton size="small" @click="filters.clientKeyId = row.observation.clientKeyId; search()">
                  Key
                </ZButton>
              </div>
            </template>
          </ZTable>
          <ZTable v-else :columns="groupColumns" :data="groups" :loading="loading" empty-text="当前页没有匹配的分组">
            <template #label="{ row }">
              <span class="block truncate" :title="view === 'accounts' ? row.sessions[0]?.accountId : row.label">{{ row.label }}</span>
            </template>
            <template #header-selection>
              <ZCheckbox :model-value="allSelected" :indeterminate="selectedSessions.length > 0 && !allSelected" :disabled="saving" aria-label="选择本页全部会话" @update:model-value="toggle(sessions, $event)" />
            </template>
            <template #selection="{ row }">
              <ZCheckbox :model-value="row.sessions.every(session => selected.has(key(session)))" :indeterminate="row.sessions.some(session => selected.has(key(session))) && !row.sessions.every(session => selected.has(key(session)))" :disabled="saving" :aria-label="`选择本页 ${row.label} 的会话`" @update:model-value="toggle(row.sessions, $event)" />
            </template>
            <template #count="{ row }">
              {{ row.sessions.length }}
            </template><template #actions="{ row }">
              <ZButton size="small" @click="inspect(row)">
                查看会话
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
    <ZDialog v-model="showOperation" title="亲和操作预览与结果" width="56rem" :show-close="!saving" :close-on-click-modal="!saving" :close-on-press-escape="!saving">
      <template v-if="operation">
        <p class="mt-0" :title="operation.plan.directive.action === 'target' ? operation.plan.directive.accountId : undefined">
          {{ directiveLabel(operation.plan.directive) }} · {{ operation.plan.items.length }} 个会话
        </p>
        <p class="text-cp-sm text-cp-text-secondary">
          预览有效至 {{ operation.plan.expiresAtDisplay }}。执行时复核账号范围和绑定版本，冲突项不会覆盖。正在处理的请求继续完成，原生续接保留其账号归属。目标账号还需通过实际请求的模型、健康和容量检查
        </p>
        <ul class="m-0 grid max-h-96 list-none gap-3 overflow-auto p-0">
          <li v-for="item in operation.plan.items" :key="key(item.session)" class="rounded-cp-lg border border-cp-border p-3">
            <p class="m-0 text-cp-sm">
              {{ callerLabel(item.session) }} · <span :title="item.session.accountId">{{ accountLabel(item.session.accountId) }}</span>
            </p><p class="my-1 truncate font-mono text-cp-xs text-cp-text-tertiary" :title="item.session.reference">
              {{ item.session.reference }}
            </p>
            <p v-if="operation.results[key(item.session)]" class="m-0 text-cp-sm" role="status">
              {{ operation.results[key(item.session)]!.message }}{{ operation.results[key(item.session)]!.auditRecorded ? '' : '（审计待补写，请重试）' }}
            </p>
            <p v-else class="m-0 text-cp-sm" :class="item.rejection ? 'text-cp-error' : 'text-cp-text-secondary'">
              {{ item.rejection ?? '待执行' }}
            </p>
          </li>
        </ul>
      </template>
      <template #footer>
        <ZButton :disabled="saving" @click="showOperation = false">
          关闭
        </ZButton><ZButton type="primary" :loading="saving" :disabled="!operation" @click="apply">
          {{ operation && Object.keys(operation.results).length ? '重试 / 补写审计' : '确认执行' }}
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
