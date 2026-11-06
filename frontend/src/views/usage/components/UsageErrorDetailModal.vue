<script setup lang="ts">
import type { UsageRecordDetail } from '@/api'
import { ZButton, ZDialog } from '@codex-proxy/ui'
import { computed } from 'vue'
import { RouterLink } from 'vue-router'
import { usageErrorSummary } from '@/components/usage/shared/errors'
import { usageAccountText, usageModelDisplay, usageTransportType } from '@/components/usage/shared/presenter'
import { formatDuration } from '@/utils/format'
import { attemptOutcomeClass, attemptOutcomeText, visibleResponseText } from '../utils/detail'
import RequestDiagnosticsPanel from './RequestDiagnosticsPanel.vue'
import UsageDetailCodePanel from './UsageDetailCodePanel.vue'
import UsageDetailFieldGrid from './UsageDetailFieldGrid.vue'
import UsageDetailState from './UsageDetailState.vue'
import UsageStatusCodeBadge from './UsageStatusCodeBadge.vue'

const props = defineProps<{ record: UsageRecordDetail | null, loading?: boolean, error?: string }>()
const emit = defineEmits<{ retry: [] }>()
const open = defineModel<boolean>({ default: false })

const panelClass = 'min-w-0 rounded-cp-card bg-cp-fill-quaternary px-4 py-3.5'
const panelTitleClass = 'm-0 text-cp-sm leading-none font-heavy text-cp-text-secondary'

const errorMessage = computed(() => {
  const message = props.record?.message?.trim()
  return message && message !== props.record?.logicalOutcome
    ? message
    : '未记录具体错误消息'
})
const errorSummary = computed(() => props.record
  ? usageErrorSummary(props.record)
  : null)
const upstreamMessage = computed(() => props.record?.upstreamError?.message)
const errorItems = computed(() => [
  { label: 'Code', value: props.record?.upstreamError?.code ?? props.record?.errorCode },
  { label: 'Type', value: props.record?.upstreamError?.errorType },
  { label: 'Param', value: props.record?.upstreamError?.param },
  {
    label: 'WS 关闭码',
    value: props.record?.websocketCloseCode,
  },
  {
    label: '最后事件',
    value: props.record?.upstreamError?.lastEventType,
  },
  {
    label: '重试等待',
    value: props.record?.retryAfterMs == null
      ? null
      : formatDuration(props.record.retryAfterMs),
  },
].filter(item => item.value != null && item.value !== '').map(item => ({ ...item, mono: true, wrap: true })))
const responseText = computed(() => props.record
  ? visibleResponseText(props.record)
  : '')
const metadataText = computed(() => props.record && Object.keys(props.record.metadata).length
  ? JSON.stringify(props.record.metadata, null, 2)
  : '')
const contextItems = computed(() => [
  {
    label: '账号',
    value: props.record
      ? usageAccountText(props.record)
      : null,
    mono: true,
    fullWidth: true,
  },
  { label: '时间', value: props.record?.createdAtDisplay, mono: true },
  { label: '总尝试', value: props.record?.attemptCount, mono: true },
  { label: '总耗时', value: props.record?.latencyMsDisplay, mono: true },
  { label: '平台', value: props.record?.provider, mono: true },
  { label: '端点', value: props.record?.route, mono: true },
  {
    label: '请求模型',
    value: props.record
      ? usageModelDisplay(props.record).primary
      : null,
    mono: true,
  },
  { label: '上游模型', value: props.record?.upstreamModel, mono: true },
  {
    label: '客户端传输',
    value: usageTransportType(props.record?.clientTransport),
    mono: true,
  },
  {
    label: '上游传输',
    value: usageTransportType(props.record?.upstreamTransport),
    mono: true,
  },
])
const identifierItems = computed(() => [
  ...(props.record?.callerIdentity?.mode === 'newApi'
    ? [
        {
          label: '身份来源 ID',
          value: props.record.callerIdentity.sourceId,
          mono: true,
          wrap: true,
        },
        {
          label: 'New API 用户 ID',
          value: props.record.callerIdentity.userId,
          mono: true,
          wrap: true,
        },
        {
          label: 'New API Token ID',
          value: props.record.callerIdentity.tokenId,
          mono: true,
          wrap: true,
        },
        ...(props.record.callerTokenName
          ? [
              {
                label: 'New API Token 名称',
                value: props.record.callerTokenName,
                mono: true,
                wrap: true,
              },
            ]
          : []),
      ]
    : []),
  { label: '客户端状态码', value: props.record?.clientStatusCode, mono: true },
  { label: '上游状态码', value: props.record?.upstreamStatusCode, mono: true },
  {
    label: '请求 ID',
    value: props.record?.requestId,
    mono: true,
    fullWidth: true,
  },
  ...(props.record?.upstreamRequestId
    ? [
        {
          label: '上游请求 ID',
          value: props.record.upstreamRequestId,
          mono: true,
          fullWidth: true,
        },
      ]
    : []),
  {
    label: '响应 ID',
    value: props.record?.responseId,
    mono: true,
    fullWidth: true,
  },
  {
    label: '客户端 Key ID',
    value: props.record?.clientApiKeyId,
    mono: true,
    fullWidth: true,
  },
])
const attempts = computed(() => (props.record?.attempts ?? []).map(attempt => ({
  id: attempt.id,
  index: attempt.attemptIndex,
  outcome: attempt.outcome,
  statusCode: attempt.statusCode,
  items: [
    { label: '错误码', value: attempt.providerErrorCode, mono: true },
    { label: '失败分类', value: attempt.failureClass, mono: true },
    {
      label: '账号',
      value: usageAccountText(attempt),
      mono: true,
    },
    { label: '平台', value: attempt.provider, mono: true },
    {
      label: '上游传输',
      value: usageTransportType(attempt.transport),
      mono: true,
    },
    {
      label: '耗时',
      value: formatDuration(attempt.latencyMs),
      mono: true,
    },
    { label: '触发方式', value: attempt.trigger, mono: true },
    { label: '发送状态', value: attempt.sendState, mono: true },
  ],
})))
</script>

<template>
  <ZDialog v-model="open" title="错误详情" description="失败原因与请求上下文" type="danger" width="64rem">
    <UsageDetailState v-if="loading || error" :loading="!!loading" :error="error" variant="error" @retry="emit('retry')" />
    <div v-else-if="record" class="grid min-w-0 gap-3">
      <section class="min-w-0 rounded-cp-card bg-cp-error-container px-4 py-3.5 text-cp-error-on-container">
        <div class="flex items-center gap-3">
          <h3 class="m-0 text-cp-sm leading-none font-heavy">
            {{ record.logicalOutcome === 'incomplete' ? '响应未完成' : '请求失败' }}
          </h3>
          <span v-if="record.recovered" class="rounded-cp-sm bg-cp-success-container px-2 py-1 text-cp-xs font-emphasis text-cp-success-on-container">
            后续已恢复
          </span>
        </div>
        <p class="mt-3 mb-0 wrap-anywhere whitespace-pre-wrap font-mono text-cp-sm leading-relaxed">
          {{ errorSummary || errorMessage }}
        </p>
        <UsageDetailFieldGrid v-if="errorItems.length" :items="errorItems" />
        <template v-if="upstreamMessage">
          <p class="mt-3 mb-1 text-cp-xs text-cp-text-secondary">
            Message
          </p>
          <p class="m-0 wrap-anywhere whitespace-pre-wrap font-mono text-cp-sm leading-relaxed" translate="no">
            {{ upstreamMessage }}
          </p>
        </template>
        <template v-if="record.upstreamError?.closeReason">
          <p class="mt-3 mb-1 text-cp-xs text-cp-text-secondary">
            关闭原因
          </p>
          <p class="m-0 wrap-anywhere whitespace-pre-wrap font-mono text-cp-sm leading-relaxed" translate="no">
            {{ record.upstreamError.closeReason }}
          </p>
        </template>
        <template v-if="!upstreamMessage || upstreamMessage !== errorMessage">
          <p class="mt-3 mb-1 text-cp-xs text-cp-text-secondary">
            网关摘要
          </p>
          <p class="m-0 wrap-anywhere whitespace-pre-wrap font-mono text-cp-sm leading-relaxed" translate="no">
            {{ errorMessage }}
          </p>
        </template>
      </section>

      <section class="grid min-w-0 gap-3 lg:grid-cols-2">
        <section :class="panelClass">
          <h3 :class="panelTitleClass">
            请求上下文
          </h3>
          <UsageDetailFieldGrid :items="contextItems" />
        </section>
        <section :class="panelClass">
          <h3 :class="panelTitleClass">
            错误定位
          </h3>
          <UsageDetailFieldGrid :items="identifierItems" />
        </section>
      </section>

      <section v-if="attempts.length || !record.attemptsComplete" :class="panelClass">
        <h3 :class="panelTitleClass">
          尝试链路
        </h3>
        <p v-if="!record.attemptsComplete" class="mt-3 mb-0 text-cp-sm text-cp-text-tertiary">
          尝试记录可能不完整
        </p>
        <ol v-if="attempts.length" class="mt-3 mb-0 grid list-none gap-4 p-0 lg:grid-cols-2">
          <li v-for="attempt in attempts" :key="attempt.id" class="min-w-0">
            <div class="flex items-center gap-3 text-cp-sm font-bold">
              <span>尝试 {{ attempt.index }}</span>
              <span :class="attemptOutcomeClass(attempt.outcome)">{{ attemptOutcomeText(attempt.outcome) }}</span>
              <UsageStatusCodeBadge :status-code="attempt.statusCode" />
            </div>
            <UsageDetailFieldGrid :items="attempt.items" />
          </li>
        </ol>
      </section>

      <details v-if="responseText || metadataText" :class="panelClass">
        <summary class="cursor-pointer text-cp-sm font-heavy text-cp-text-secondary">
          诊断数据
        </summary>
        <div class="mt-3 grid min-w-0 gap-3">
          <UsageDetailCodePanel v-if="responseText" title="响应内容" max-height="180px" :content="responseText" />
          <UsageDetailCodePanel v-if="metadataText" title="元数据" max-height="240px" :content="metadataText" />
        </div>
      </details>
      <RequestDiagnosticsPanel :request-id="record.requestId" :active="open" />
    </div>

    <template #footer>
      <RouterLink v-if="record?.callerIdentity?.mode === 'newApi'" :to="{ path: '/affinity', query: { sourceId: record.callerIdentity.sourceId, userId: record.callerIdentity.userId, tokenId: record.callerIdentity.tokenId } }" class="mr-auto text-cp-sm text-cp-link">
        查看调用方亲和
      </RouterLink>
      <ZButton type="primary" @click="open = false">
        关闭
      </ZButton>
    </template>
  </ZDialog>
</template>
