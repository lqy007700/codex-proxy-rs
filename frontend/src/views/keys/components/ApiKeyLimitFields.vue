<script setup lang="ts">
import type { ApiKeyLimitField } from '../composables/useApiKeyFormRules'
import { ZButton, ZFormItem, ZInput } from '@codex-proxy/ui'
import { DollarSign } from '@lucide/vue'
import { computed } from 'vue'

defineProps<{ disabled: boolean, mixed?: ApiKeyLimitField[] }>()
const emit = defineEmits<{ edit: [field: ApiKeyLimitField] }>()
const dailyLimitUsd = defineModel<string>('dailyLimitUsd', { required: true })
const weeklyLimitUsd = defineModel<string>('weeklyLimitUsd', { required: true })
const maxConcurrency = defineModel<string>('maxConcurrency', { required: true })
const requestsPerMinute = defineModel<string>('requestsPerMinute', { required: true })

const fields = computed(() => [
  {
    key: 'dailyLimitUsd' as const,
    model: dailyLimitUsd,
    label: '日限额',
    accessible: '日限额（美元）',
    money: true,
  },
  {
    key: 'weeklyLimitUsd' as const,
    model: weeklyLimitUsd,
    label: '周限额',
    accessible: '周限额（美元）',
    money: true,
  },
  {
    key: 'maxConcurrency' as const,
    model: maxConcurrency,
    label: '最大并发',
    accessible: '最大并发',
    money: false,
  },
  {
    key: 'requestsPerMinute' as const,
    model: requestsPerMinute,
    label: '每分钟请求数（RPM）',
    accessible: '每分钟请求数',
    money: false,
  },
])

function update(field: typeof fields.value[number], value: string) {
  field.model.value = value
  emit('edit', field.key)
}
</script>

<template>
  <div class="grid gap-6 sm:grid-cols-2">
    <ZFormItem v-for="field in fields" :key="field.key" :prop="field.key" :label="field.label">
      <template v-if="mixed?.includes(field.key)" #label-extra>
        <span class="text-z-xs text-z-text-secondary">多个值</span>
      </template>
      <template v-if="mixed?.includes(field.key)" #extra>
        <ZButton variant="link" size="small" :disabled="disabled" @click="update(field, '')">
          设为不限
        </ZButton>
      </template>
      <ZInput
        :model-value="field.model.value"
        type="number"
        min="0"
        :step="field.money ? 'any' : '1'"
        :aria-label="field.accessible"
        placeholder="不限制"
        :disabled="disabled"
        @update:model-value="update(field, $event)"
      >
        <template v-if="field.money" #prefix>
          <DollarSign class="size-4" aria-hidden="true" />
        </template>
      </ZInput>
    </ZFormItem>
  </div>
</template>
