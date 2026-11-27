<script setup lang="ts">
import type { OpenAiRequestSettings } from '@/api'
import { ZCard, ZForm, ZFormItem, ZInput, ZSelect } from '@codex-proxy/ui'
import { routingFields, routingHeaderError } from '../openaiOverrides'

defineProps<{ disabled: boolean }>()

type RoutingMode = 'inherit' | 'disabled' | 'custom'

const settings = defineModel<OpenAiRequestSettings>({ required: true })
const modes: Array<{ value: RoutingMode, label: string }> = [
  { value: 'inherit', label: '继承配置' },
  { value: 'disabled', label: '不发送' },
  { value: 'custom', label: '自定义' },
]

function modeFor(value: OpenAiRequestSettings[keyof OpenAiRequestSettings]): RoutingMode {
  if (value === undefined)
    return 'inherit'
  return value === null
    ? 'disabled'
    : 'custom'
}

function setMode(key: keyof OpenAiRequestSettings, mode: RoutingMode | undefined) {
  if (!mode || mode === modeFor(settings.value[key]))
    return
  const next = { ...settings.value }
  if (mode === 'inherit') {
    delete next[key]
  }
  else {
    next[key] = mode === 'disabled'
      ? null
      : ''
  }
  settings.value = next
}
</script>

<template>
  <ZCard title="OpenAI 路由请求头" description="继承配置时使用 YAML 或内置默认值，自定义可发送空字符串">
    <ZForm class="max-w-6xl sm:grid-cols-2">
      <div v-for="field in routingFields" :key="field.key" class="grid min-w-0 content-start gap-3">
        <ZFormItem :label="field.label" :description="field.header">
          <ZSelect
            :model-value="modeFor(settings[field.key])"
            :options="modes"
            :aria-label="field.label"
            :disabled="disabled"
            @update:model-value="setMode(field.key, $event)"
          />
        </ZFormItem>
        <ZFormItem v-if="typeof settings[field.key] === 'string'" :error="routingHeaderError(settings[field.key])">
          <ZInput
            :model-value="settings[field.key] ?? ''"
            :aria-label="`${field.label}请求头值`"
            placeholder="请求头值"
            :disabled="disabled"
            @update:model-value="settings = { ...settings, [field.key]: $event }"
          />
        </ZFormItem>
      </div>
    </ZForm>
  </ZCard>
</template>
