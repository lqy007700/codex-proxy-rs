import type { UsageErrorFacts } from '@/api/modules/usage'

export type UsageErrorRecord = UsageErrorFacts & {
  provider: string | null
  logicalOutcome: string
}

// 只解释已记录的分类和关闭码，原始上游文案由详情保留
export function usageErrorSummary(record: UsageErrorRecord): string | null {
  if (record.websocketCloseCode != null) {
    if (record.websocketCloseCode === 1012)
      return '上游关闭连接（服务重启）'
    if (record.websocketCloseCode === 1013)
      return '上游关闭连接（稍后重试）'
    if (record.websocketCloseCode === 1009)
      return '上游因消息过大关闭连接'
    return '上游在响应完成前关闭连接'
  }

  if (record.errorKind === 'process_interrupted')
    return '请求执行中断，未记录到最终结果'

  if (record.errorKind === 'continuation_recovery_required') {
    if (record.continuationUnavailableReason === 'reused_connection_lost'
      && record.upstreamConnectionExitReason === 'max_age_expired') {
      return '续接连接已到期，需要完整重发'
    }
    return '续接状态不可用，需要完整重发'
  }

  if (record.provider === 'openai') {
    switch (record.errorCode) {
      case 'previous_response_not_found':
        return '续接状态不可用，需要完整重发'
      case 'websocket_connection_limit_reached':
        return '上游连接已到时限，需要重新连接'
      case 'context_length_exceeded':
        return '上下文超过模型限制'
      case 'insufficient_quota':
        return '上游可用额度不足'
      case 'rate_limit_exceeded':
        return '上游请求频率受限'
      case 'server_is_overloaded':
        return '上游服务负载过高'
    }
  }

  if (record.upstreamConnectionExitReason === 'upstream_close')
    return '上游在响应完成前关闭连接'
  return null
}

export function usageErrorCode(record: UsageErrorFacts): string | null {
  return record.websocketCloseCode != null
    ? `WS ${record.websocketCloseCode}`
    : record.errorCode
}
