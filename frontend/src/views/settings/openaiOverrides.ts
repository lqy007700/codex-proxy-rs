import type { OpenAiRequestSettings, OpenAiWebsocketPool } from '@/api'

export const routingFields = [
  { key: 'residency', label: '数据驻留', header: 'x-openai-internal-codex-residency' },
  { key: 'accountRoutingOverride', label: '账号路由覆盖', header: 'x-openai-account-routing-override' },
] as const

export function routingHeaderError(value: OpenAiRequestSettings[keyof OpenAiRequestSettings]): string {
  return typeof value === 'string' && !/^[\t\x20-\x7E]*$/.test(value)
    ? '请使用 ASCII 请求头值'
    : ''
}

export const websocketPoolFields = [
  {
    key: 'maxAgeMs',
    label: '连接最大寿命',
    help: '单条上游连接可复用的最长时间',
    min: 1,
    max: Number.MAX_SAFE_INTEGER,
  },
  {
    key: 'maxConnecting',
    label: '并发建连上限',
    help: '单个网关进程同时建立的上游连接数',
    min: 1,
    max: 4_294_967_295,
  },
  {
    key: 'fastPathBudgetMs',
    label: '建连等待时间',
    help: '允许回退 HTTP 的请求等待建连的时间，可填 1～15000 毫秒',
    min: 1,
    max: 15_000,
  },
  {
    key: 'idleTimeoutMs',
    label: '空闲回收时间',
    help: '普通空闲连接在池中可保留的时间',
    min: 1,
    max: Number.MAX_SAFE_INTEGER,
  },
  {
    key: 'streamIdleTimeoutMs',
    label: '流式空闲超时',
    help: '等待下一条上游业务消息的时间，填 0 使用内置超时',
    min: 0,
    max: Number.MAX_SAFE_INTEGER,
  },
] as const

export type WebsocketPoolDraft = Record<keyof OpenAiWebsocketPool, string>

export function websocketPoolDraft(settings: OpenAiWebsocketPool = {}): WebsocketPoolDraft {
  return {
    maxAgeMs: String(settings.maxAgeMs ?? ''),
    maxConnecting: String(settings.maxConnecting ?? ''),
    fastPathBudgetMs: String(settings.fastPathBudgetMs ?? ''),
    idleTimeoutMs: String(settings.idleTimeoutMs ?? ''),
    streamIdleTimeoutMs: String(settings.streamIdleTimeoutMs ?? ''),
  }
}

export function parseWebsocketPool(draft: WebsocketPoolDraft) {
  const overrides: OpenAiWebsocketPool = {}
  const errors: Partial<WebsocketPoolDraft> = {}
  for (const { key, min, max } of websocketPoolFields) {
    const raw = draft[key].trim()
    if (!raw)
      continue
    const value = Number(raw)
    if (!/^\d+$/.test(raw) || value < min || value > max) {
      errors[key] = `请输入 ${min}～${max} 之间的整数`
    }
    else {
      overrides[key] = value
    }
  }
  return { overrides, errors }
}
