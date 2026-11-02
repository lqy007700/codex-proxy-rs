import type { AccountUpstream } from '@/api'

const failureLabels: Record<NonNullable<AccountUpstream['lastError']>, string> = {
  unauthorized: '互联未授权',
  unsupported: '普通上游',
  rate_limited: '互联同步受限',
  invalid_response: '互联响应无效',
  unavailable: '互联同步失败',
}

export function upstreamStatus(info: AccountUpstream) {
  const label = info.lastError
    ? failureLabels[info.lastError]
    : info.discovered
      ? (info.stale ? '互联观测已过期' : 'CPR 互联')
      : '互联待识别'
  return {
    label,
    title: info.observedAt
      ? `${label}，观测时间 ${info.observedAtDisplay}`
      : label,
    warning: info.stale || Boolean(info.lastError && info.lastError !== 'unsupported'),
  }
}
