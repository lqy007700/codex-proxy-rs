<script setup lang="ts">
import type { FormRule } from '@codex-proxy/ui'
import { ZFormItem, ZInput } from '@codex-proxy/ui'

defineProps<{ disabled?: boolean, formProp?: string, label?: string, showMessage?: boolean }>()

const country = defineModel<string>({ required: true })
const countryRule: FormRule = {
  required: true,
  pattern: /^[A-Za-z]{2}$/u,
  transform: value => value.trim(),
  trigger: ['blur', 'change'],
  message: '请输入两位国家代码',
}
</script>

<template>
  <ZFormItem :prop="formProp" :rules="countryRule" :show-message="showMessage" :label="label ?? '国家代码'" required>
    <ZInput v-model="country" :disabled="disabled" :aria-label="label ?? '国家代码'" placeholder="请输入两位国家代码" maxlength="2" />
  </ZFormItem>
</template>
