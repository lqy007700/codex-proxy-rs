<script setup lang="ts">
import type { AlertSettingsForm } from '../../composables/useAlertSettings'

import { ZCard, ZCheckbox, ZForm, ZFormItem, ZIconButton, ZInput, ZPopover, ZSwitch } from '@codex-proxy/ui'
import { CircleHelp } from '@lucide/vue'

defineProps<{ disabled: boolean }>()
const form = defineModel<AlertSettingsForm>({ required: true })
</script>

<template>
  <ZCard title="模型请求失败率">
    <template #actions>
      <ZPopover trigger="hover-click" placement="bottom-start">
        <template #reference="{ open }">
          <ZIconButton aria-label="失败率统计口径说明" :title="undefined" :aria-expanded="open">
            <CircleHelp class="size-4" />
          </ZIconButton>
        </template>
        <div class="max-w-80 space-y-2 px-3 py-2 text-cp-sm leading-relaxed text-cp-text-secondary">
          <p>按完成时间统计模型请求，最终失败与异常中断计为失败</p>
          <p>内部重试后成功只计一次成功，主动取消、预热和内部调用不参与统计</p>
          <p>请求记录异步写入，可能延迟或丢失，不覆盖执行前的鉴权与准入拒绝</p>
          <p>无请求、样本不足或检查失败不会视为恢复，网关停机时无法自行发送通知</p>
        </div>
      </ZPopover>
    </template>
    <ZForm class="max-w-6xl sm:grid-cols-2">
      <ZSwitch v-model="form.enabled" label="启用失败率告警" :disabled="disabled" class="col-span-full justify-self-start" />
      <ZFormItem label="统计周期" description="每分钟检查最近一段时间的请求">
        <ZInput v-model="form.windowMinutes" aria-label="统计周期" type="number" min="1" max="60" :disabled="disabled">
          <template #suffix>
            <span class="text-cp-sm">分钟</span>
          </template>
        </ZInput>
      </ZFormItem>
      <ZFormItem label="最小请求数" description="不足此数量时不触发，也不判定恢复">
        <ZInput v-model="form.minimumRequests" aria-label="最小请求数" type="number" min="1" max="1000000" :disabled="disabled">
          <template #suffix>
            <span class="text-cp-sm">次</span>
          </template>
        </ZInput>
      </ZFormItem>
      <ZFormItem label="触发阈值" description="失败率达到此值时视为异常">
        <ZInput v-model="form.failureThresholdPercent" aria-label="触发阈值" type="number" min="2" max="100" :disabled="disabled">
          <template #suffix>
            <span class="text-cp-sm">%</span>
          </template>
        </ZInput>
      </ZFormItem>
      <ZFormItem label="连续触发次数" description="相邻滑动窗口连续异常后发送通知">
        <ZInput v-model="form.consecutiveEvaluations" aria-label="连续触发次数" type="number" min="1" max="10" :disabled="disabled">
          <template #suffix>
            <span class="text-cp-sm">次</span>
          </template>
        </ZInput>
      </ZFormItem>
      <ZFormItem label="恢复阈值" description="低于此值连续两次后恢复，需小于触发阈值">
        <ZInput v-model="form.recoveryThresholdPercent" aria-label="恢复阈值" type="number" min="1" max="99" :disabled="disabled">
          <template #suffix>
            <span class="text-cp-sm">%</span>
          </template>
        </ZInput>
      </ZFormItem>
      <ZFormItem label="持续异常提醒间隔">
        <template #extra>
          <ZCheckbox v-model="form.reminderEnabled" label="重复提醒" :disabled="disabled" />
        </template>
        <ZInput v-model="form.reminderMinutes" aria-label="持续异常提醒间隔" type="number" min="5" max="1440" :disabled="disabled || !form.reminderEnabled">
          <template #suffix>
            <span class="text-cp-sm">分钟</span>
          </template>
        </ZInput>
      </ZFormItem>
      <ZCheckbox v-model="form.recoveryNotification" label="恢复后发送邮件" :disabled="disabled" class="col-span-full justify-self-start" />
    </ZForm>
  </ZCard>
</template>
