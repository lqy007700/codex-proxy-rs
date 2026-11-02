<script setup lang="ts">
import type { Account, AccountQuotaWindow } from '@/api'
import { ZButton } from '@codex-proxy/ui'
import { computed, shallowRef, watch } from 'vue'
import { getAccountDetail } from '@/api'
import { useRequestState } from '@/composables/useRequestState'
import AccountQuotaPanelEntry from './Entry.vue'

const props = defineProps<{ account: Account }>()
const expanded = shallowRef(false)
const detail = shallowRef<Account | null>(null)
const page = shallowRef(1)
const pageSize = 25
const request = useRequestState()
const { loading, error } = request
const pool = computed(() => props.account.upstream?.pool)
const nodes = computed(() => {
  const groups = new Map<string, AccountQuotaWindow[]>()
  for (const window of detail.value?.quota.windows ?? []) {
    if (!window.sourceAccountId)
      continue
    const windows = groups.get(window.sourceAccountId) ?? []
    windows.push(window)
    groups.set(window.sourceAccountId, windows)
  }
  return [...groups].map(([id, windows]) => ({ id, windows }))
})
const pages = computed(() => Math.max(1, Math.ceil(nodes.value.length / pageSize)))
const visibleNodes = computed(() => nodes.value.slice((page.value - 1) * pageSize, page.value * pageSize))

async function load() {
  const id = request.start()
  const accountId = props.account.id
  const connection = props.account.upstream?.connectionRevision
  try {
    const result = await getAccountDetail({ accountId }, { signal: request.signal, silent: true })
    // 列表切换、出口或凭据变更后，迟到的详情不能重新显示旧节点
    if (request.isCurrent(id) && result.account.upstream?.connectionRevision === connection)
      detail.value = result.account
  }
  catch (cause) {
    request.fail(id, cause)
  }
  finally {
    request.finish(id)
  }
}

function toggle() {
  expanded.value = !expanded.value
  if (expanded.value && !detail.value)
    void load()
  else if (!expanded.value)
    request.invalidate()
}

watch(() => [props.account.id, props.account.upstream?.connectionRevision, props.account.upstream?.observedAt], () => {
  request.invalidate()
  detail.value = null
  page.value = 1
  if (expanded.value)
    void load()
})
</script>

<template>
  <section class="grid min-w-0 gap-3" aria-label="上游账号池">
    <div v-if="pool" class="flex flex-wrap items-baseline gap-x-4 gap-y-1 text-cp-xs text-cp-text-secondary">
      <span>账号池 <strong class="font-mono text-cp-text">{{ pool.states.ready }} / {{ pool.accountCount }}</strong> 就绪</span>
      <span v-if="pool.states.exhausted">耗尽 {{ pool.states.exhausted }}</span>
      <span v-if="pool.states.cooldown">冷却 {{ pool.states.cooldown }}</span>
      <span v-if="pool.states.disabled">停用 {{ pool.states.disabled }}</span>
      <span v-if="pool.states.error">错误 {{ pool.states.error }}</span>
      <span v-if="pool.states.unknown">未知 {{ pool.states.unknown }}</span>
      <span v-for="plan in pool.plans" :key="plan.name">{{ plan.name }} {{ plan.count }}</span>
    </div>
    <ZButton size="small" :aria-expanded="expanded" @click="toggle">
      {{ expanded ? '收起节点额度' : '查看节点额度' }}
    </ZButton>
    <div v-if="expanded" class="grid min-w-0 gap-3">
      <p v-if="loading" role="status" class="text-cp-xs text-cp-text-secondary">
        正在读取节点额度
      </p>
      <div v-else-if="error" role="alert" class="grid gap-2 text-cp-xs text-cp-error-text">
        <p>{{ error }}</p>
        <ZButton size="small" @click="load">
          重试
        </ZButton>
      </div>
      <template v-else>
        <p v-if="!nodes.length" class="text-cp-xs text-cp-text-secondary">
          上游未提供节点额度观测
        </p>
        <section v-for="(node, index) in visibleNodes" :key="node.id" class="grid gap-2 rounded-lg bg-cp-fill-quaternary p-3">
          <h4 class="text-cp-xs font-bold text-cp-text-secondary">
            节点 {{ (page - 1) * pageSize + index + 1 }}
          </h4>
          <AccountQuotaPanelEntry :label="null" :windows="node.windows" />
        </section>
        <nav v-if="pages > 1" class="flex items-center justify-between gap-3" aria-label="节点额度分页">
          <ZButton size="small" :disabled="page === 1" @click="page--">
            上一页
          </ZButton>
          <span class="text-cp-xs text-cp-text-secondary">{{ page }} / {{ pages }}</span>
          <ZButton size="small" :disabled="page === pages" @click="page++">
            下一页
          </ZButton>
        </nav>
      </template>
    </div>
  </section>
</template>
