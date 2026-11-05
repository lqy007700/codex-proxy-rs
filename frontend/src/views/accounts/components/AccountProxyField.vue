<script setup lang="ts">
import type { OutboundProxyTest } from '@/api'
import { ZFormItem, ZIconButton, ZSelect } from '@codex-proxy/ui'
import { Wifi } from '@lucide/vue'
import { computed } from 'vue'
import { useProxyCatalog } from '@/composables/useProxyCatalog'
import { useAccountProxyTest } from '../composables/useAccountProxyTest'
import AccountProxyTestBadge from './AccountProxyTestBadge.vue'

const props = withDefaults(defineProps<{ accountId?: string, currentProxyId?: string | null, endpoint?: string | null, disabled?: boolean, preserve?: boolean, error?: string }>(), { preserve: true })
const mode = defineModel<string>('mode', { required: true })
const proxyId = defineModel<string>('proxyId', { required: true })
const { proxies, loading, loadProxies } = useProxyCatalog()
// 按绑定 ID 识别代理，避免相同脱敏地址下的不同认证配置被误认
const currentSelection = computed(() => props.currentProxyId || (props.endpoint ? 'preserve' : 'direct'))
const options = computed(() => [
  ...(props.preserve && !props.accountId
    ? [
        {
          label: '保持当前',
          value: 'preserve',
          description: '保留各账号的连接设置',
        },
      ]
    : []),
  ...(props.preserve && props.accountId && props.endpoint && !proxies.value.some(proxy => proxy.id === props.currentProxyId)
    ? [
        { label: props.endpoint, value: currentSelection.value },
      ]
    : []),
  { label: '直连', value: 'direct', description: '不使用代理' },
  ...proxies.value.map(proxy => ({
    label: `${proxy.name.trim() || proxy.endpoint}${!proxy.enabled
      ? '（已禁用）'
      : proxy.lastTest?.success
        ? ''
        : proxy.lastTest ? '（测试失败）' : '（未测试）'}`,
    value: proxy.id,
    description: proxy.name.trim()
      ? proxy.endpoint
      : undefined,
    disabled: !proxy.enabled,
  })),
])
const selection = computed({
  get: () => mode.value === 'proxy'
    ? proxyId.value
    : mode.value === 'preserve' && props.accountId
      ? currentSelection.value
      : mode.value,
  set: (value: string) => {
    // 显示已绑定代理，未改选择时仍省略写入，保留禁用代理及仅地址配置
    const nextMode = props.preserve && props.accountId && value === currentSelection.value
      ? 'preserve'
      : value === 'preserve' || value === 'direct'
        ? value
        : 'proxy'
    mode.value = nextMode
    proxyId.value = nextMode === 'proxy'
      ? value
      : ''
  },
})
const selectedProxy = computed(() => proxies.value.find(proxy => proxy.id === selection.value))
// 目录刷新只禁用测试入口，不改变目标，避免清除版本冲突提示
const testTarget = computed(() => {
  if (props.disabled)
    return null
  // 禁用代理的账号实际走直连，不把该代理自身的测试结果当成账号出口
  if (selection.value === 'direct' || selectedProxy.value?.enabled === false)
    return 'direct' as const
  return selectedProxy.value ?? null
})
const { result, directResult, testing, test } = useAccountProxyTest({
  target: () => testTarget.value,
  accountId: () => props.accountId,
  onConflict: loadProxies,
  onTested: (record) => {
    proxies.value = proxies.value.map(proxy => proxy.id === record.id
      ? record
      : proxy)
  },
})
// 代理复用目录中的最近测试，直连结果只保留在当前账号的编辑过程
const optionTests = computed<Record<string, OutboundProxyTest | null>>(() => ({
  ...Object.fromEntries(proxies.value.map(proxy => [
    proxy.id,
    proxy.enabled ? proxy.lastTest : null,
  ])),
  direct: directResult.value,
}))
const selectedTest = computed(() => result.value ?? (testTarget.value === 'direct'
  ? directResult.value
  : optionTests.value[selection.value]))
const testHint = computed(() => selectedProxy.value?.enabled === false
  ? '代理已禁用，将测试服务器直连出口'
  : selection.value !== 'direct' && !selectedProxy.value
    ? '选择直连或受管理的代理后可测试出口'
    : null)
</script>

<template>
  <ZFormItem label="出站隧道" :error="error">
    <div class="flex items-center gap-2">
      <ZSelect v-model="selection" class="min-w-0 flex-1" :options="options" :disabled="disabled || loading" aria-label="出站隧道" />
      <ZIconButton aria-label="测试出站连接" :loading="testing" :disabled="disabled || loading || !testTarget" @click="test">
        <Wifi class="size-4" />
      </ZIconButton>
    </div>
    <AccountProxyTestBadge v-if="selectedTest" :result="selectedTest" />
    <p v-if="testHint" class="m-0 mt-1 text-cp-xs text-cp-text-tertiary">
      {{ testHint }}
    </p>
  </ZFormItem>
</template>
