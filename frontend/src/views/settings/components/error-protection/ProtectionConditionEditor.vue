<script setup lang="ts">
import type { ProtectionCondition, ProtectionField, ProtectionOperator } from '@/api/modules/settings/protection'
import { ZButton, ZCheckbox, ZIconButton, ZInput, ZPopover, ZSelect } from '@codex-proxy/ui'
import { ChevronDown, Plus, Trash2, X } from '@lucide/vue'
import { computed, useId } from 'vue'
import { protectionConditionError, protectionFieldLabels, protectionLimits, protectionOperatorLabels } from './form'

const props = defineProps<{ disabled: boolean }>()
const emit = defineEmits<{ remove: [] }>()
const condition = defineModel<ProtectionCondition>({ required: true })
const optionsId = useId()
const errorId = useId()
const error = computed(() => protectionConditionError(condition.value))
const hasMissing = computed(() => condition.value.values.includes(null))
const fields = computed(() => ([
  'provider',
  'status',
  'code',
  'type',
  'message',
] as ProtectionField[]).map(value => ({
  value,
  label: protectionFieldLabels[value],
})))
const operators = computed(() => (condition.value.field === 'message'
  ? ['equals', 'contains'] as const
  : ['equals'] as const).map(value => ({
  value,
  label: protectionOperatorLabels[value],
})))

function update(patch: Partial<ProtectionCondition>) {
  if (!props.disabled) {
    condition.value = {
      ...condition.value,
      ...patch,
    }
  }
}

function changeField(field: ProtectionField) {
  update({
    field,
    operator: field === 'message'
      ? condition.value.operator
      : 'equals',
  })
}

function changeOperator(operator: ProtectionOperator) {
  update({ operator })
}

function updateValue(index: number, value: string) {
  update({
    values: condition.value.values.map((previous, i) => i === index
      ? value
      : previous),
  })
}

function removeValue(index: number) {
  const values = condition.value.values.filter((_, i) => i !== index)
  update({
    values: values.length
      ? values
      : [''],
  })
}

function toggleMissing(missing: boolean) {
  if (missing) {
    if (!hasMissing.value && condition.value.values.length < protectionLimits.values) {
      update({
        values: [...condition.value.values, null],
      })
    }
  }
  else {
    // 空文本与缺失分别匹配，只移除缺失候选，保留用户输入的空文本
    const values = condition.value.values.filter(value => value !== null)
    update({
      values: values.length
        ? values
        : [''],
    })
  }
}
</script>

<template>
  <div class="grid min-w-0 grid-cols-[minmax(0,1fr)_auto] items-start gap-2 rounded-cp bg-cp-bg-container p-3 md:grid-cols-[9rem_7rem_minmax(0,1fr)_auto]">
    <div class="grid min-w-0 grid-cols-2 gap-2 md:col-span-2 md:grid-cols-[9rem_7rem]">
      <ZSelect
        :model-value="condition.field"
        :options="fields"
        :disabled="disabled"
        aria-label="匹配字段"
        @update:model-value="changeField($event as ProtectionField)"
      />
      <ZSelect
        :model-value="condition.operator"
        :options="operators"
        :disabled="disabled"
        aria-label="匹配方式"
        @update:model-value="changeOperator($event as ProtectionOperator)"
      />
    </div>
    <div class="col-span-full grid min-w-0 gap-1 md:col-span-1 md:col-start-3 md:row-start-1" role="group" aria-label="任意候选值匹配即可">
      <div class="grid gap-2">
        <div v-for="(value, index) in condition.values" :key="index" class="flex min-w-0 items-center gap-2">
          <span v-if="condition.values.length > 1" class="w-4 shrink-0 text-center text-cp-xs text-cp-text-tertiary">
            {{ index > 0 ? '或' : '' }}
          </span>
          <span v-if="value === null" class="flex min-h-9 min-w-0 flex-1 items-center rounded-cp bg-cp-fill-quaternary px-3 text-cp-sm text-cp-text-secondary">
            字段缺失
          </span>
          <ZInput
            v-else
            :model-value="value"
            class="min-w-0 flex-1 font-mono"
            placeholder="输入匹配值"
            :disabled="disabled"
            :aria-label="`候选值 ${index + 1}`"
            :aria-invalid="Boolean(error)"
            :aria-describedby="error ? errorId : undefined"
            :inputmode="condition.field === 'status' ? 'numeric' : undefined"
            @update:model-value="updateValue(index, $event)"
          />
          <ZIconButton v-if="condition.values.length > 1 || value === null" size="small" :disabled="disabled" :aria-label="`删除候选值 ${index + 1}`" @click="removeValue(index)">
            <X class="size-3.5" />
          </ZIconButton>
        </div>
      </div>
      <div class="flex flex-wrap items-center justify-between gap-x-3 gap-y-1" :class="condition.values.length > 1 ? 'pl-6' : undefined">
        <ZButton size="small" variant="text" :disabled="disabled || condition.values.length >= protectionLimits.values" @click="update({ values: [...condition.values, ''] })">
          <template #icon>
            <Plus class="size-3.5" />
          </template>
          添加候选值
        </ZButton>
        <ZPopover trigger="click" placement="bottom-end" :teleported="false" :disabled="disabled">
          <template #reference="{ open }">
            <ZButton size="small" variant="text" :disabled="disabled" :aria-expanded="open" :aria-controls="open ? optionsId : undefined">
              {{ condition.ignoreCase ? '忽略大小写' : '匹配选项' }}
              <ChevronDown class="size-3.5" />
            </ZButton>
          </template>
          <div :id="optionsId" class="grid max-w-72 gap-3 p-3 text-cp-sm">
            <ZCheckbox :model-value="condition.ignoreCase" :disabled="disabled" label="忽略 ASCII 大小写" @update:model-value="update({ ignoreCase: $event })" />
            <ZCheckbox
              v-if="condition.operator === 'equals' || hasMissing"
              :model-value="hasMissing"
              :disabled="disabled || (!hasMissing && condition.values.length >= protectionLimits.values)"
              label="字段缺失"
              @update:model-value="toggleMissing"
            />
            <p class="m-0 leading-relaxed text-cp-text-tertiary">
              字段缺失与空文本不同，等值比较忽略两端空白，忽略大小写仅适用于 ASCII 字母
            </p>
            <p v-if="condition.field === 'message'" class="m-0 leading-relaxed text-cp-text-tertiary">
              包含匹配检查错误信息的前 8 KiB，保留完整字符，等值匹配比较完整信息，超长信息不会被视为字段缺失
            </p>
          </div>
        </ZPopover>
      </div>
    </div>
    <ZIconButton class="col-start-2 row-start-1 md:col-start-4" :disabled="disabled" aria-label="删除条件" @click="emit('remove')">
      <Trash2 class="size-4" />
    </ZIconButton>
    <p v-if="error" :id="errorId" role="alert" class="col-span-full m-0 text-cp-sm text-cp-error-text">
      {{ error }}
    </p>
  </div>
</template>
