<script setup lang="ts">
import type { AlertSample, AlertStatus } from '@/api'

import { defineTableColumns, ZButton, ZCard, ZTable } from '@codex-proxy/ui'
import { RefreshCw } from '@lucide/vue'
import { deliveryLabels, evaluationLabel, eventLabels, notificationLabels, phaseLabel, rateLabel } from './presenter'

defineProps<{
  status: AlertStatus | null
  preview: AlertSample | null
  loading: boolean
  error: string
  previewing: boolean
  actionDisabled: boolean
}>()
defineEmits<{ refresh: [], preview: [] }>()

const eventColumns = defineTableColumns<AlertStatus['events'][number]>([
  { key: 'status', label: '事件状态', kind: 'status' },
  { key: 'openedAtDisplay', label: '触发时间', kind: 'datetime' },
  { key: 'closedAtDisplay', label: '结束时间', kind: 'datetime' },
  { key: 'failureRate', label: '触发失败率', kind: 'numeric' },
  { key: 'requests', label: '有效请求', kind: 'numeric' },
])
const deliveryColumns = defineTableColumns<AlertStatus['deliveries'][number]>([
  { key: 'kind', label: '通知类型', kind: 'status' },
  { key: 'recipient', label: '接收邮箱', kind: 'text' },
  { key: 'status', label: '投递状态', kind: 'status' },
  { key: 'attempts', label: '尝试次数', kind: 'numeric' },
  { key: 'createdAtDisplay', label: '创建时间', kind: 'datetime' },
  { key: 'result', label: '发送结果', kind: 'text' },
])
</script>

<template>
  <ZCard title="运行状态">
    <template #actions>
      <div class="flex flex-wrap items-center gap-2">
        <ZButton :loading="previewing" :disabled="actionDisabled" @click="$emit('preview')">
          预览当前窗口
        </ZButton>
        <ZButton :loading="loading" :disabled="loading || actionDisabled" @click="$emit('refresh')">
          <template #icon>
            <RefreshCw class="size-4" />
          </template>
          刷新状态
        </ZButton>
      </div>
    </template>
    <p v-if="error" role="alert" class="mb-4 text-cp-sm text-cp-error-text">
      状态读取失败，当前显示上次读取结果，可稍后刷新重试
    </p>
    <dl class="grid gap-x-8 gap-y-3 text-cp-sm sm:grid-cols-2 lg:grid-cols-3">
      <div class="flex items-baseline justify-between gap-3">
        <dt class="text-cp-text-secondary">
          告警状态
        </dt>
        <dd :class="status?.runtime.phase === 'firing' ? 'text-cp-error-text' : undefined">
          {{ phaseLabel(status?.runtime) }}
        </dd>
      </div>
      <div class="flex items-baseline justify-between gap-3">
        <dt class="text-cp-text-secondary">
          最近评估
        </dt>
        <dd>{{ evaluationLabel(status?.runtime) }}</dd>
      </div>
      <div class="flex items-baseline justify-between gap-3">
        <dt class="text-cp-text-secondary">
          最近检查
        </dt>
        <dd>{{ status?.runtime.checkedAtDisplay ?? '—' }}</dd>
      </div>
      <div class="flex items-baseline justify-between gap-3">
        <dt class="text-cp-text-secondary">
          有效请求
        </dt>
        <dd>{{ status?.runtime.sample?.requests ?? '—' }}</dd>
      </div>
      <div class="flex items-baseline justify-between gap-3">
        <dt class="text-cp-text-secondary">
          失败请求
        </dt>
        <dd>{{ status?.runtime.sample?.failures ?? '—' }}</dd>
      </div>
      <div class="flex items-baseline justify-between gap-3">
        <dt class="text-cp-text-secondary">
          失败率
        </dt>
        <dd>{{ rateLabel(status?.runtime.sample?.failureRatePercent) }}</dd>
      </div>
    </dl>
    <p v-if="status?.runtime.sample" class="mt-3 text-cp-sm text-cp-text-secondary">
      统计窗口 {{ status.runtime.sample.windowStartDisplay }} 至 {{ status.runtime.sample.windowEndDisplay }}
    </p>
    <div v-if="preview" role="status" class="mt-4 space-y-1 rounded-cp bg-cp-fill-quaternary p-3 text-cp-sm">
      <p>窗口预览：有效请求 {{ preview.requests }} 次，失败 {{ preview.failures }} 次，失败率 {{ rateLabel(preview.failureRatePercent) }}</p>
      <p class="text-cp-text-secondary">
        {{ preview.windowStartDisplay }} 至 {{ preview.windowEndDisplay }}，预览不触发通知或改变告警状态
      </p>
    </div>
  </ZCard>

  <ZCard title="最近告警" description="最近 10 条事件，配置变更结束不代表业务恢复">
    <ZTable :columns="eventColumns" :data="status?.events ?? []" row-key="id" :loading="loading && !status" empty-text="暂无告警事件">
      <template #status="{ row }">
        <span :class="row.status === 'firing' ? 'text-cp-error-text' : undefined">{{ eventLabels[row.status] }}</span>
      </template>
      <template #closedAtDisplay="{ row }">
        {{ row.closedAtDisplay ?? '—' }}
      </template>
      <template #failureRate="{ row }">
        {{ rateLabel(row.sample.failureRatePercent) }}
      </template>
      <template #requests="{ row }">
        {{ row.sample.requests }}
      </template>
    </ZTable>
  </ZCard>

  <ZCard title="邮件投递" description="最近 10 条收件人投递结果，发送失败不改变告警状态">
    <ZTable :columns="deliveryColumns" :data="status?.deliveries ?? []" row-key="id" :loading="loading && !status" empty-text="暂无邮件投递记录">
      <template #kind="{ row }">
        {{ notificationLabels[row.kind] }}
      </template>
      <template #status="{ row }">
        <span :class="row.status === 'failed' ? 'text-cp-error-text' : undefined">{{ deliveryLabels[row.status] }}</span>
      </template>
      <template #result="{ row }">
        <span v-if="row.sentAtDisplay">{{ row.sentAtDisplay }}</span>
        <span v-else-if="row.status === 'pending' && row.attempts">下次重试 {{ row.nextAttemptAtDisplay }}</span>
        <span v-else>{{ row.errorMessage ?? '—' }}</span>
      </template>
    </ZTable>
  </ZCard>
</template>
