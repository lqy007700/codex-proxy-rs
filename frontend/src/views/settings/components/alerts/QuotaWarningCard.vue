<script setup lang="ts">
import type { AlertSettingsForm } from '../../composables/useAlertSettings'
import type { QuotaWarningPreview, QuotaWarningStatus } from '@/api'
import { defineTableColumns, ZButton, ZCard, ZForm, ZFormItem, ZInput, ZSwitch, ZTable } from '@codex-proxy/ui'
import { rateLabel } from './presenter'

defineProps<{
  disabled: boolean
  enabled: boolean
  status: QuotaWarningStatus | null
  preview: QuotaWarningPreview | null
  previewing: boolean
  previewDisabled: boolean
}>()
defineEmits<{ preview: [] }>()
const form = defineModel<AlertSettingsForm>({ required: true })
const columns = defineTableColumns<QuotaWarningPreview['items'][number]>([
  { key: 'accountLabel', label: '账号', kind: 'text' },
  { key: 'period', label: '额度窗口', kind: 'text' },
  { key: 'usedPercent', label: '已使用', kind: 'numeric' },
  { key: 'threshold', label: '预警阈值', kind: 'numeric' },
  { key: 'resetAtDisplay', label: '预计重置', kind: 'datetime' },
  { key: 'observedAtDisplay', label: '额度更新时间', kind: 'datetime' },
])
</script>

<template>
  <ZCard title="OAuth 额度预警">
    <template #actions>
      <ZButton :loading="previewing" :disabled="previewDisabled" @click="$emit('preview')">
        预览额度预警
      </ZButton>
    </template>
    <ZForm class="max-w-6xl sm:grid-cols-2">
      <ZSwitch v-model="form.quotaWarningEnabled" label="启用额度预警" :disabled="disabled" class="col-span-full justify-self-start" />
      <ZFormItem label="5 小时额度使用阈值">
        <ZInput v-model="form.shortTermThresholdPercent" aria-label="5 小时额度使用阈值" type="number" min="1" max="100" :disabled="disabled">
          <template #suffix>
            <span class="text-cp-sm">%</span>
          </template>
        </ZInput>
      </ZFormItem>
      <ZFormItem label="周额度使用阈值">
        <ZInput v-model="form.weeklyThresholdPercent" aria-label="周额度使用阈值" type="number" min="1" max="100" :disabled="disabled">
          <template #suffix>
            <span class="text-cp-sm">%</span>
          </template>
        </ZInput>
      </ZFormItem>
    </ZForm>
    <p class="mt-3 text-cp-sm leading-relaxed text-cp-text-secondary">
      每 10 分钟读取已有额度数据，不主动查询上游，每个窗口每周期提醒一次，同轮合并发送，复用下方邮件配置
    </p>
    <p class="mt-1 text-cp-sm text-cp-text-secondary">
      覆盖已启用的 OpenAI OAuth 普通及保底账号，仅检查通用 5 小时和周额度，缺失或过期数据跳过
    </p>
    <dl class="mt-4 grid gap-x-8 gap-y-2 text-cp-sm sm:grid-cols-2 lg:grid-cols-3">
      <div class="flex justify-between gap-3">
        <dt class="text-cp-text-secondary">
          检查状态
        </dt>
        <dd>{{ !enabled ? '未启用' : status?.error ? '检查失败' : status?.stale ? '检查结果已过期' : status?.checkedAtDisplay ? '检查完成' : '等待首次检查' }}</dd>
      </div>
      <div class="flex justify-between gap-3">
        <dt class="text-cp-text-secondary">
          最近检查
        </dt>
        <dd>{{ status?.checkedAtDisplay ?? '—' }}</dd>
      </div>
      <div class="flex justify-between gap-3">
        <dt class="text-cp-text-secondary">
          本轮预警窗口
        </dt>
        <dd>{{ status?.pendingWindows ?? '—' }}</dd>
      </div>
      <div class="flex justify-between gap-3">
        <dt class="text-cp-text-secondary">
          已检查账号
        </dt>
        <dd>{{ status?.accounts ?? '—' }}</dd>
      </div>
      <div class="flex justify-between gap-3">
        <dt class="text-cp-text-secondary">
          本周期已提醒窗口
        </dt>
        <dd>{{ status?.notifiedWindows ?? '—' }}</dd>
      </div>
      <div class="flex justify-between gap-3">
        <dt class="text-cp-text-secondary">
          数据不可用
        </dt>
        <dd>{{ status?.unavailableAccounts ?? 0 }} 个账号 / {{ status?.unavailableWindows ?? 0 }} 个窗口</dd>
      </div>
    </dl>
    <div v-if="preview" class="mt-4 space-y-3" role="status">
      <p class="text-cp-sm text-cp-text-secondary">
        当前有 {{ preview.status.pendingWindows }} 个窗口待提醒，预览不发送邮件或消耗提醒次数
      </p>
      <ZTable :columns="columns" :data="preview.items" :row-key="row => `${row.accountId}:${row.windowKey}`" empty-text="暂无待提醒额度窗口">
        <template #period="{ row }">
          {{ row.period === 'short_term' ? '5 小时额度' : '周额度' }}
        </template>
        <template #usedPercent="{ row }">
          {{ rateLabel(row.usedPercent) }}
        </template>
        <template #threshold="{ row }">
          {{ row.threshold }}%
        </template>
      </ZTable>
    </div>
  </ZCard>
</template>
