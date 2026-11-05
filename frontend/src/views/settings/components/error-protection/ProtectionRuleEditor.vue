<script setup lang="ts">
import type { ProtectionCondition, ProtectionRule } from '@/api/modules/settings/protection'
import { ZButton } from '@codex-proxy/ui'
import { Plus } from '@lucide/vue'
import { computed, toRaw } from 'vue'
import { createProtectionCondition, protectionLimits, protectionRuleConditionsError } from './form'
import ProtectionConditionEditor from './ProtectionConditionEditor.vue'

const props = defineProps<{ disabled: boolean, showErrors: boolean }>()
const rule = defineModel<ProtectionRule>({ required: true })
const conditionKeys = new WeakMap<object, number>()
let nextConditionKey = 0
function conditionKey(condition: ProtectionCondition) {
  const raw = toRaw(condition)
  let key = conditionKeys.get(raw)
  if (key === undefined) {
    key = nextConditionKey++
    conditionKeys.set(raw, key)
  }
  return key
}
const error = computed(() => protectionRuleConditionsError(rule.value.conditions))

function update(patch: Partial<ProtectionRule>) {
  if (!props.disabled) {
    rule.value = {
      ...rule.value,
      ...patch,
    }
  }
}

function updateCondition(index: number, condition: ProtectionCondition) {
  // 对象更新沿用编辑身份，删除前面的条件时也不串用后续条件的输入状态
  conditionKeys.set(toRaw(condition), conditionKey(rule.value.conditions[index]!))
  update({
    conditions: rule.value.conditions.map((previous, i) => i === index
      ? condition
      : previous),
  })
}
</script>

<template>
  <div class="grid gap-4 px-3 pb-4 sm:px-4">
    <div class="grid gap-2">
      <span class="text-cp-sm font-emphasis text-cp-text-secondary">满足全部条件</span>
      <template v-for="(condition, index) in rule.conditions" :key="conditionKey(condition)">
        <span v-if="index > 0" class="pl-3 text-cp-xs font-emphasis text-cp-text-secondary">且</span>
        <ProtectionConditionEditor
          :model-value="condition"
          :disabled="disabled"
          :show-errors="showErrors"
          @update:model-value="updateCondition(index, $event)"
          @remove="update({ conditions: rule.conditions.filter((_, i) => i !== index) })"
        />
      </template>
      <ZButton
        class="justify-self-start"
        size="small"
        variant="text"
        :disabled="disabled || rule.conditions.length >= protectionLimits.conditions"
        @click="update({ conditions: [...rule.conditions, createProtectionCondition()] })"
      >
        <template #icon>
          <Plus class="size-3.5" />
        </template>
        添加条件
      </ZButton>
    </div>
    <p v-if="showErrors && error" class="m-0 text-cp-sm text-cp-error-text" role="alert">
      {{ error }}
    </p>
  </div>
</template>
