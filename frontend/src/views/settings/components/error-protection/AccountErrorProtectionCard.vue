<script setup lang="ts">
import type { ProtectionDraft } from './form'
import type { ProtectionRule } from '@/api/modules/settings/protection'
import { ZButton, ZCard, ZCheckbox, ZForm, ZFormItem, ZIconButton, ZInput, ZPopover, ZSwitch } from '@codex-proxy/ui'

import { Activity, CircleAlert, Gauge, Plus, Snowflake, Timer } from '@lucide/vue'

import { computed, shallowRef, useId } from 'vue'
import { generateRequestId } from '@/utils/operation'
import { createProtectionCondition, protectionLimits } from './form'
import ProtectionRuleRow from './ProtectionRuleRow.vue'

const props = defineProps<{ disabled: boolean }>()

const policy = defineModel<ProtectionDraft>({ required: true })
const expandedId = shallowRef<string | null>(null)
const rulesError = computed(() => policy.value.enabled && !policy.value.rules.length
  ? '启用前请添加至少一条有效规则'
  : '')
const probeModelId = useId()
const helpId = useId()

function update(patch: Partial<ProtectionDraft>) {
  if (!props.disabled) {
    policy.value = {
      ...policy.value,
      ...patch,
    }
  }
}

function numericModel(key: 'threshold' | 'windowSeconds' | 'durationSeconds') {
  return computed({
    get: () => policy.value[key] === null
      ? ''
      : String(policy.value[key]),
    set: (value: string) => update({
      [key]: value.trim() && Number.isFinite(Number(value))
        ? Number(value)
        : null,
    }),
  })
}

const threshold = numericModel('threshold')
const windowSeconds = numericModel('windowSeconds')
const durationSeconds = numericModel('durationSeconds')

function addRule() {
  const rule: ProtectionRule = {
    id: generateRequestId(),
    name: '',
    conditions: [
      createProtectionCondition(),
    ],
  }
  update({
    rules: [...policy.value.rules, rule],
  })
  expandedId.value = rule.id
}

function updateRule(rule: ProtectionRule) {
  update({
    rules: policy.value.rules.map(previous => previous.id === rule.id
      ? rule
      : previous),
  })
}

function removeRule(id: string) {
  update({
    rules: policy.value.rules.filter(rule => rule.id !== id),
  })
  if (expandedId.value === id)
    expandedId.value = null
}
</script>

<template>
  <ZCard
    title="错误保护"
    description="匹配规则的失败累计达到阈值时，暂停账号调度并进入冷却"
  >
    <ZForm class="max-w-6xl sm:grid-cols-2 sm:items-end">
      <ZSwitch
        :model-value="policy.enabled"
        :disabled="disabled"
        class="col-span-full justify-self-start"
        label="启用错误保护"
        @update:model-value="update({ enabled: $event })"
      />

      <section class="col-span-full grid min-w-0 gap-3" aria-label="匹配规则">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <div class="flex min-w-0 items-center gap-1">
            <span class="text-cp-sm font-emphasis text-cp-text-secondary">命中任意规则后累计</span>
            <ZPopover trigger="hover-click" placement="top-start" :hover-delay="240">
              <template #reference="{ open }">
                <ZIconButton
                  size="small"
                  aria-label="匹配规则说明"
                  :aria-expanded="open"
                  :aria-describedby="open ? helpId : undefined"
                >
                  <CircleAlert class="size-3.5" />
                </ZIconButton>
              </template>
              <div :id="helpId" class="grid max-w-80 gap-2 px-3 py-2 text-cp-sm leading-relaxed text-cp-text-secondary">
                <p class="m-0">
                  规则内需满足全部条件，每个条件命中任意候选值即可，同一次失败只累计一次
                </p>
                <p class="m-0">
                  字段缺失与空文本不同，等值比较忽略两端空白，忽略大小写仅适用于 ASCII 字母
                </p>
                <p class="m-0">
                  最多 32 条规则，每条 8 个条件，每个条件 16 个候选值，所有规则文本与探测模型合计不超过 64 KiB UTF-8
                </p>
              </div>
            </ZPopover>
          </div>
          <ZButton size="small" :disabled="disabled || policy.rules.length >= protectionLimits.rules" @click="addRule">
            <template #icon>
              <Plus class="size-3.5" />
            </template>
            添加规则
          </ZButton>
        </div>
        <p v-if="!policy.rules.length" class="m-0 rounded-cp bg-cp-fill-quaternary px-4 py-4 text-cp-sm text-cp-text-tertiary">
          暂无规则，添加规则以选择要累计的失败
        </p>
        <div v-else class="grid min-w-0 gap-2">
          <template v-for="(rule, index) in policy.rules" :key="rule.id">
            <span v-if="index > 0" class="pl-4 text-cp-xs font-emphasis text-cp-text-tertiary">或</span>
            <ProtectionRuleRow
              :rule="rule"
              :expanded="expandedId === rule.id"
              :disabled="disabled"
              @toggle="expandedId = expandedId === rule.id ? null : rule.id"
              @remove="removeRule(rule.id)"
              @update="updateRule"
            />
          </template>
        </div>
        <p v-if="rulesError" class="m-0 text-cp-sm text-cp-error-text" role="alert">
          {{ rulesError }}
        </p>
      </section>

      <ZFormItem
        label="失败阈值"
        description="所有规则命中的失败共同累计"
      >
        <ZInput
          v-model="threshold"
          :disabled="disabled || !policy.enabled"
          aria-label="失败阈值"
          type="number"
          min="2"
          max="1000"
          step="1"
        >
          <template #prefix>
            <Activity class="size-4" />
          </template>
          <template #suffix>
            <span class="text-cp-sm">次</span>
          </template>
        </ZInput>
      </ZFormItem>

      <ZFormItem
        label="计数有效期"
        description="每次匹配失败后续期，普通调用成功后清零"
      >
        <ZInput
          v-model="windowSeconds"
          :disabled="disabled || !policy.enabled"
          aria-label="计数有效期秒数"
          type="number"
          min="60"
          max="3600"
          step="1"
        >
          <template #prefix>
            <Timer class="size-4" />
          </template>
          <template #suffix>
            <span class="text-cp-sm">秒</span>
          </template>
        </ZInput>
      </ZFormItem>

      <ZFormItem
        label="冷却时长"
        description="探测失败时，按此时长延后恢复"
      >
        <ZInput
          v-model="durationSeconds"
          :disabled="disabled || !policy.enabled"
          aria-label="冷却时长秒数"
          type="number"
          min="300"
          max="604800"
          step="1"
        >
          <template #prefix>
            <Snowflake class="size-4" />
          </template>
          <template #suffix>
            <span class="text-cp-sm">秒</span>
          </template>
        </ZInput>
      </ZFormItem>
      <ZFormItem
        label="恢复探测模型"
        description="留空选用首个可用模型"
        :for="probeModelId"
      >
        <template #extra>
          <ZCheckbox
            :model-value="policy.probeEnabled"
            :disabled="disabled || !policy.enabled"
            label="探测成功后恢复"
            @update:model-value="update({ probeEnabled: $event })"
          />
        </template>
        <ZInput
          :id="probeModelId"
          :model-value="policy.probeModel"
          :disabled="disabled || !policy.enabled || !policy.probeEnabled"
          aria-label="恢复探测模型"
          placeholder="留空自动选择"
          @update:model-value="update({ probeModel: $event })"
        >
          <template #prefix>
            <Gauge class="size-4" />
          </template>
        </ZInput>
      </ZFormItem>
    </ZForm>
  </ZCard>
</template>
