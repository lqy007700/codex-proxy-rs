<script setup lang="ts">
import type { ResetWindowWeights } from '@/api'
import { ZButton, ZDialog, ZFormItem, ZInput } from '@codex-proxy/ui'
import { computed, reactive, shallowRef, useId, watch } from 'vue'

const props = defineProps<{ config: ResetWindowWeights, defaults: ResetWindowWeights }>()
const emit = defineEmits<{ confirm: [config: ResetWindowWeights] }>()
const open = defineModel<boolean>({ required: true })
const formId = useId()
const fields = [
  { key: 'fiveHour', label: '5 小时窗口系数' },
  { key: 'weekly', label: '周窗口系数' },
  { key: 'monthly', label: '月窗口系数' },
] as const
const draft = reactive({ fiveHour: '', weekly: '', monthly: '' })
const submitted = shallowRef(false)
const errors = computed(() => Object.fromEntries(fields.map(({ key }) => {
  const value = Number(draft[key])
  const valid = draft[key].trim() && Number.isFinite(value) && value >= 0 && value <= 10 && Math.round(value * 10) / 10 === value
  return [
    key,
    valid ? undefined : '请输入 0～10 的数值，最多一位小数',
  ]
})))
function replaceDraft(config: ResetWindowWeights) {
  for (const { key } of fields)
    draft[key] = String(config[key])
  submitted.value = false
}
watch(open, (value) => {
  if (value)
    replaceDraft(props.config)
}, { immediate: true })
function confirm() {
  submitted.value = true
  if (Object.values(errors.value).some(Boolean))
    return
  emit('confirm', {
    fiveHour: Number(draft.fiveHour),
    weekly: Number(draft.weekly),
    monthly: Number(draft.monthly),
  })
  open.value = false
}
</script>

<template>
  <ZDialog v-model="open" title="额度重置优先设置" description="调整各额度窗口的重置偏好" width="36rem">
    <form :id="formId" class="grid gap-4" novalidate @submit.prevent="confirm">
      <p class="m-0 text-z-sm text-z-text-secondary">
        全部为 0 时按最近重置时间排序
      </p>
      <div class="grid gap-4 sm:grid-cols-3">
        <ZFormItem v-for="field in fields" :key="field.key" :label="field.label" :error="submitted ? errors[field.key] : undefined">
          <ZInput v-model="draft[field.key]" type="number" min="0" max="10" step="0.1" :aria-label="field.label" />
        </ZFormItem>
      </div>
    </form>
    <template #footer>
      <ZButton class="mr-auto" @click="replaceDraft(defaults)">
        恢复默认
      </ZButton>
      <ZButton @click="open = false">
        取消
      </ZButton>
      <ZButton native-type="submit" :form="formId" type="primary">
        确定
      </ZButton>
    </template>
  </ZDialog>
</template>
