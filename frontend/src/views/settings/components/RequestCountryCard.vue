<script setup lang="ts">
import type { OpenAiRequestCountry } from '@/api'
import { ZCard, ZFormItem, ZSelect } from '@codex-proxy/ui'
import CountryCodeField from '@/components/CountryCodeField.vue'
import HelpPopover from '@/components/HelpPopover.vue'

defineProps<{ disabled: boolean }>()

const requestCountry = defineModel<OpenAiRequestCountry>({ required: true })
const modes: Array<{ value: OpenAiRequestCountry['mode'], label: string, help: string }> = [
  {
    value: 'follow_location',
    label: '跟随位置',
    help: '优先使用代理位置，其次使用已开启的全局位置。两者都未设置时发送 null',
  },
  {
    value: 'default',
    label: '空值',
    help: '始终发送 JSON null',
  },
  {
    value: 'passthrough',
    label: '透传',
    help: '保留客户端原值，未传时省略该字段',
  },
  {
    value: 'custom',
    label: '自定义',
    help: '固定使用指定的两位国家代码',
  },
]

function setMode(mode: string | undefined) {
  const next = mode as OpenAiRequestCountry['mode'] | undefined
  if (next && next !== requestCountry.value.mode) {
    requestCountry.value = next === 'custom'
      ? { mode: next, country: '' }
      : { mode: next }
  }
}
</script>

<template>
  <ZCard description="设置 Codex 请求如何发送国家代码">
    <template #title>
      <span class="inline-flex items-center gap-1.5">
        请求国家
        <HelpPopover label="请求国家模式说明">
          <p class="mb-2 font-emphasis text-z-text">
            仅用于 Codex OAuth 账号
          </p>
          <dl class="m-0 grid grid-cols-[auto_1fr] gap-x-3 gap-y-2">
            <template v-for="mode in modes" :key="mode.value">
              <dt class="font-emphasis text-z-text">
                {{ mode.label }}
              </dt>
              <dd class="m-0">{{ mode.help }}</dd>
            </template>
          </dl>
        </HelpPopover>
      </span>
    </template>
    <div class="grid min-w-0 max-w-6xl items-start gap-4 sm:grid-cols-2">
      <ZFormItem label="策略">
        <ZSelect :model-value="requestCountry.mode" :options="modes" aria-label="策略" :disabled="disabled" @update:model-value="setMode" />
      </ZFormItem>
      <CountryCodeField
        v-if="requestCountry.mode === 'custom'"
        :model-value="requestCountry.country"
        label="自定义国家代码"
        :show-message="Boolean(requestCountry.country.trim())"
        :disabled="disabled"
        @update:model-value="requestCountry = { mode: 'custom', country: $event }"
      />
    </div>
  </ZCard>
</template>
