<script setup lang="ts">
import type { ApiKeyAccountForm } from '../utils/upstreamApiKey'
import { ZFormItem, ZInput, ZSegmented } from '@codex-proxy/ui'
import { computed } from 'vue'

defineProps<{ disabled?: boolean, editing?: boolean }>()
const form = defineModel<ApiKeyAccountForm>({ required: true })
const protocolOptions = [
  { label: 'Responses', value: 'responses' },
  { label: 'Messages', value: 'messages' },
]
const transportOptions = computed(() => [
  { label: 'HTTP / SSE', value: 'http' },
  { label: 'WS 优先', value: 'prefer_websocket', disabled: form.value.protocol === 'messages' },
])

function updateProtocol(protocol: string) {
  if (protocol !== 'responses' && protocol !== 'messages')
    return
  form.value = {
    ...form.value,
    protocol,
    transport: protocol === 'messages'
      ? 'http'
      : form.value.transport,
  }
}
</script>

<template>
  <div class="grid gap-4">
    <ZFormItem v-if="!editing" label="账号名称" required>
      <ZInput v-model="form.name" placeholder="输入上游名称" :disabled="disabled" />
    </ZFormItem>
    <ZFormItem label="Base URL" required>
      <template #extra>
        <span class="text-z-xs text-z-text-quaternary" title="填写 API 前缀，接口路径会自动追加">API 前缀</span>
      </template>
      <ZInput v-model="form.base_url" type="url" placeholder="https://api.example.com/v1" :disabled="disabled" autocomplete="off" />
    </ZFormItem>
    <ZFormItem label="API Key" :required="!editing">
      <ZInput v-model="form.apiKey" type="password" :placeholder="editing ? '留空保留当前密钥' : '输入上游密钥'" :disabled="disabled" autocomplete="new-password" />
    </ZFormItem>
    <ZFormItem label="上游协议">
      <ZSegmented :model-value="form.protocol" aria-label="上游协议" :options="protocolOptions" :disabled="disabled" @update:model-value="updateProtocol" />
    </ZFormItem>
    <ZFormItem label="传输方式">
      <ZSegmented v-model="form.transport" aria-label="传输方式" :options="transportOptions" :disabled="disabled" :title="form.protocol === 'messages' ? 'Messages 仅支持 HTTP / SSE' : 'WS 优先需要上游支持 Responses WebSocket'" />
    </ZFormItem>
  </div>
</template>
