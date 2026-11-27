<script setup lang="ts">
import type { WebsocketPoolDraft } from '../openaiOverrides'
import { ZCard, ZForm, ZFormItem, ZInput } from '@codex-proxy/ui'
import HelpPopover from '@/components/HelpPopover.vue'
import { websocketPoolFields } from '../openaiOverrides'

defineProps<{ disabled: boolean, errors: Partial<WebsocketPoolDraft> }>()

const pool = defineModel<WebsocketPoolDraft>({ required: true })
</script>

<template>
  <ZCard title="OpenAI WebSocket 连接池" description="留空时使用 YAML 或内置默认值，连接池开关由 YAML 的 openai.ws_pool.enabled 控制">
    <ZForm class="max-w-6xl sm:grid-cols-2 xl:grid-cols-3">
      <ZFormItem v-for="field in websocketPoolFields" :key="field.key" :label="field.label" :error="errors[field.key]">
        <template #label-extra>
          <HelpPopover :label="`${field.label}说明`" :content="field.help" />
        </template>
        <ZInput
          :model-value="pool[field.key]"
          :aria-label="field.key === 'maxConnecting' ? field.label : `${field.label}（毫秒）`"
          inputmode="numeric"
          placeholder="继承配置"
          :disabled="disabled"
          @update:model-value="pool = { ...pool, [field.key]: $event }"
        >
          <template v-if="field.key !== 'maxConnecting'" #suffix>
            ms
          </template>
        </ZInput>
      </ZFormItem>
    </ZForm>
  </ZCard>
</template>
