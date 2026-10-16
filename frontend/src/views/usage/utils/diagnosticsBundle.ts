import type { OpsError, UsageRecordDetail } from '@/api'
import { failureClassText } from './opsErrorPresentation'

const precommitReleaseReasons = new Set(['semantic_output', 'terminal', 'byte_limit', 'grace_timeout', 'eof'])
const retrySendStates = new Set(['NotSent', 'Sent', 'Ambiguous'])
const ordinaryRetryBlockers = new Set([
  'connection_budget_exhausted',
  'connection_retry_requested',
  'execution_effect_observed',
  'required_account',
  'continuation_active',
  'replay_barrier_reached',
  'delivery_pending',
  'ambiguous_send',
  'provider_replay_unproven',
  'routing_budget_exhausted',
])

function booleanValue(value: unknown) {
  return typeof value === 'boolean' ? value : null
}

function nonNegativeInteger(value: unknown) {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 ? value : null
}

export function requestDiagnosticsBundle(
  requestId: string,
  detail: UsageRecordDetail | null,
  errorRecord?: OpsError,
) {
  // 关联请求切换后，不能把原弹窗的错误归到另一个请求。
  const record = detail?.requestId === requestId ? detail : null
  const error = errorRecord?.requestId === requestId
    ? errorRecord
    : record && ['failed', 'cancelled', 'incomplete'].includes(record.logicalOutcome)
      ? {
          ...record,
          transport: record.upstreamTransport,
          metadata: { source: 'model_request', component: 'model_request' },
        }
      : null
  const trace = record?.trace
  // 请求终态与错误事件可能来自不同尝试，不用事件的值填补终态空字段。
  const request = record ?? error

  return {
    schemaVersion: 5,
    source: 'usage.request_diagnostics',
    exportPolicy: 'allowlisted_fields_without_payloads',
    request: {
      requestId,
      source: record ? 'request_detail' : error ? 'error_record' : 'request_id_only',
      route: request?.route ?? null,
      provider: request?.provider ?? null,
      accountId: request?.accountId ?? null,
      clientTransport: request?.clientTransport ?? null,
      upstreamTransport: record ? record.upstreamTransport : error?.transport ?? null,
      upstreamRequestId: request?.upstreamRequestId ?? null,
      responseId: request?.responseId ?? null,
      statusCode: record?.statusCode ?? null,
      clientStatusCode: request?.clientStatusCode ?? null,
      upstreamStatusCode: request?.upstreamStatusCode ?? null,
      logicalOutcome: record?.logicalOutcome ?? null,
      attemptCount: record?.attemptCount ?? null,
      createdAt: request?.createdAt ?? null,
    },
    error: error
      ? {
          id: error.id,
          requestId: error.requestId,
          source: error.metadata.source,
          component: error.metadata.component,
          summary: failureClassText(error.failureClass),
          summarySource: 'failure_class',
          failureClass: error.failureClass,
          upstreamSendState: error.upstreamSendState,
          attemptIndex: error.attemptIndex,
          clientStatusCode: error.clientStatusCode,
          upstreamStatusCode: error.upstreamStatusCode,
          upstreamRequestId: error.upstreamRequestId,
          responseId: error.responseId,
          latencyMs: error.latencyMs,
          createdAt: error.createdAt,
        }
      : null,
    // 只导出失败分类、重试判定与缓存释放事实的白名单，不复制任意事件 data
    trace: trace
      ? {
          schemaVersion: trace.schemaVersion,
          startedAtUnixMs: trace.startedAtUnixMs,
          totalEvents: trace.totalEvents,
          droppedEvents: trace.droppedEvents,
          events: trace.events.map(event => ({
            sequence: event.sequence,
            lastSequence: event.lastSequence,
            elapsedMs: event.elapsedMs,
            lastElapsedMs: event.lastElapsedMs,
            attemptIndex: event.attemptIndex,
            exchangeId: event.exchangeId,
            stage: event.stage,
            count: event.count,
            failure: event.stage === 'attempt.failed'
              ? {
                  kind: event.data.kind,
                  sendState: event.data.sendState,
                  stage: event.data.diagnostic?.stage ?? null,
                  code: event.data.diagnostic?.code ?? null,
                  upstreamStatus: event.data.upstreamStatus,
                }
              : null,
            retryDecision: event.stage === 'retry.decided'
              ? {
                  retryable: booleanValue(event.data.retryable),
                  continuationRetry: booleanValue(event.data.continuationRetry),
                  sameAccountRetry: booleanValue(event.data.sameAccountRetry),
                  accountRotationRetry: booleanValue(event.data.accountRotationRetry),
                  ordinaryRetry: booleanValue(event.data.ordinaryRetry),
                  ordinaryRetryBlockers: Array.isArray(event.data.ordinaryRetryBlockers)
                    && event.data.ordinaryRetryBlockers.every(value => typeof value === 'string' && ordinaryRetryBlockers.has(value))
                    ? event.data.ordinaryRetryBlockers
                    : null,
                  providerReplaySafe: booleanValue(event.data.providerReplaySafe),
                  retryProhibited: booleanValue(event.data.retryProhibited),
                  middlewareRetrySuppressed: booleanValue(event.data.middlewareRetrySuppressed),
                  transportRecovery: booleanValue(event.data.transportRecovery),
                  transientRetry: booleanValue(event.data.transientRetry),
                  clientRetrySuppressed: booleanValue(event.data.clientRetrySuppressed),
                  rotationRetry: booleanValue(event.data.rotationRetry),
                  accountRotations: nonNegativeInteger(event.data.accountRotations),
                  connectionRetry: booleanValue(event.data.connectionRetry),
                  connectionRetries: nonNegativeInteger(event.data.connectionRetries),
                  connectionBudgetRemainingMs: nonNegativeInteger(event.data.connectionBudgetRemainingMs),
                  executionEffectObserved: booleanValue(event.data.executionEffectObserved),
                  delayMs: nonNegativeInteger(event.data.delayMs),
                  boundedTransientRetries: nonNegativeInteger(event.data.boundedTransientRetries),
                  boundedTransientRetriesRemaining: nonNegativeInteger(event.data.boundedTransientRetriesRemaining),
                  routingAttemptsRemaining: nonNegativeInteger(event.data.routingAttemptsRemaining),
                  accountRotationsRemaining: nonNegativeInteger(event.data.accountRotationsRemaining),
                  downstreamCommitted: booleanValue(event.data.downstreamCommitted),
                  replayBarrierReached: booleanValue(event.data.replayBarrierReached),
                  sendState: typeof event.data.sendState === 'string' && retrySendStates.has(event.data.sendState)
                    ? event.data.sendState
                    : null,
                }
              : null,
            clientRetry: event.stage === 'client_retry.loaded'
              ? { previousFailure: booleanValue(event.data.previousFailure) }
              : null,
            precommitRelease: event.stage === 'provider.precommit.released'
              ? {
                  reason: typeof event.data.reason === 'string' && precommitReleaseReasons.has(event.data.reason)
                    ? event.data.reason
                    : null,
                  prefetchedBytes: nonNegativeInteger(event.data.prefetchedBytes),
                  waitMs: nonNegativeInteger(event.data.waitMs),
                }
              : null,
          })),
        }
      : null,
    attempts: record?.attempts.map(attempt => ({
      id: attempt.id,
      attemptIndex: attempt.attemptIndex,
      trigger: attempt.trigger,
      provider: attempt.provider,
      transport: attempt.transport,
      sendState: attempt.sendState,
      outcome: attempt.outcome,
      downstreamCommitted: attempt.downstreamCommitted,
      statusCode: attempt.statusCode,
      failureClass: attempt.failureClass,
      accountId: attempt.accountId,
      firstTokenMs: attempt.firstTokenMs,
      latencyMs: attempt.latencyMs,
      startedAt: attempt.startedAt,
      completedAt: attempt.completedAt,
    })) ?? [],
    relatedRequests: record?.relatedRequests.map(related => ({
      requestId: related.requestId,
      relation: related.relation,
      outcome: related.outcome,
      completedAt: related.completedAt,
    })) ?? [],
    availability: {
      requestDetail: record ? 'available' : 'unavailable',
      errorSummary: error ? 'available' : 'not_available_for_selected_request',
      trace: !record
        ? 'unknown'
        : !trace
            ? 'not_recorded'
            : trace.events.some(event => ['attempt.failed', 'retry.decided', 'client_retry.loaded', 'provider.precommit.released'].includes(event.stage))
              ? 'stages_and_allowlisted_facts'
              : 'stages_only',
      traceEventsDropped: trace?.droppedEvents ?? null,
      attemptsComplete: record?.attemptsComplete ?? null,
      relatedRequests: record ? 'available' : 'unknown',
      environment: 'not_collected',
    },
    omitted: [
      'error.message',
      'error.errorDetails',
      'error.providerErrorCode',
      'attempts.providerErrorCode',
      'request.message',
      'request.errorDetails',
      'request.providerErrorCode',
      'request.metadata',
      'trace.events.data_except_allowlisted_diagnostic_facts',
      'request_and_response_headers_and_bodies',
      'user_supplied_model_names',
      'account_names_emails_and_credential_names',
      'client_ip_user_agent_and_api_key',
    ],
    notes: [
      'schemaVersion 5 仅供人工排障，null 表示未知、未采集或未通过导出校验，不代表没有发生错误',
      '错误摘要只使用稳定分类，不含上游错误码或原文，error 为 null 时请结合 attempts 的分类与状态',
      '时间线导出阶段、顺序、计时、Core 记录的失败分类、重试判定、轮次失败状态及 Provider 缓存释放事实，不含错误原文和其他事件 data',
      'retryDecision 记录重试资格、抑制、重放屏障、预算和退避判定，不代表已经发起重试；实际重试与换号需结合后续 attempt.started、attemptCount 和账号核对',
      'attemptsComplete 不为 true 时，尝试列表不完整，没有时间线的旧记录无法补回未采集事件',
      '未自动采集网关版本、客户端版本、部署环境和故障发生时区，请另行补充',
      '关联 ID 仍可能属于内部信息，分享前请审阅，原始错误、正文和日志需另行审阅脱敏，勿直接公开',
    ],
  }
}
