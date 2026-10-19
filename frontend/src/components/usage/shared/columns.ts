import type { TableColumn } from '@codex-proxy/ui'
import type { UsageListRecord } from '@/api'
import { defineTableColumns } from '@codex-proxy/ui'
import { formatProviderLabel } from '@/utils/providers'

export const usageDataColumns = [
  { key: 'model', label: '模型', kind: 'custom', size: 'lg' },
  { key: 'reasoningEffort', label: '推理强度', kind: 'status', size: 'sm', fixedWidth: true, align: 'left' },
  { key: 'route', label: '端点', kind: 'mono' },
  { key: 'upstreamTransport', label: '上游', kind: 'status', size: 'sm', fixedWidth: true },
  { key: 'clientTransport', label: '接入', kind: 'status', size: 'sm', fixedWidth: true },
  { key: 'tokenDetails', label: 'TOKEN', kind: 'numeric', size: 'xl' },
  { key: 'billing', label: '费用', kind: 'numeric', size: 'lg' },
  { key: 'latency', label: '延迟', kind: 'numeric', size: 'xl' },
  { key: 'performance', label: '速率', kind: 'numeric', size: 'lg' },
  { key: 'clientIp', label: 'IP', kind: 'custom', size: 'xl' },
  { key: 'userAgent', label: 'User-Agent', kind: 'custom', size: '2xl' },
  { key: 'createdAtDisplay', label: '时间', kind: 'datetime' },
] satisfies TableColumn[]

export const usageRecordColumns = defineTableColumns<UsageListRecord>([
  { key: 'clientApiKeyName', label: '密钥名称', kind: 'identity', size: 'lg' },
  { key: 'createdAtDisplay', label: '请求时间', kind: 'datetime', size: 'xl', fixedWidth: true },
  {
    key: 'accountEmail',
    hideable: false,
    label: '账号',
    kind: 'identity',
    size: '2xl',
  },
  { key: 'accountPlanType', label: '订阅', kind: 'status', size: 'sm', fixedWidth: true },
  {
    key: 'provider',
    label: '平台/类型',
    kind: 'status',
    size: 'md',
    fixedWidth: true,
    formatter: (value: unknown) => formatProviderLabel(typeof value === 'string' ? value : null),
  },
  ...usageDataColumns.filter(column => column.key !== 'createdAtDisplay'),
  { key: 'actions', label: '操作', kind: 'actions', size: 'sm', hideable: false },
])
