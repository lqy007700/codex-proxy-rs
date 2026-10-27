import type { RequestOptions } from '../../request'
import request from '../../request'

export interface AlertSettingsValues {
  enabled: boolean
  windowMinutes: number
  minimumRequests: number
  failureThresholdPercent: number
  consecutiveEvaluations: number
  recoveryThresholdPercent: number
  reminderMinutes: number | null
  recoveryNotification: boolean
  smtpHost: string
  smtpPort: number
  smtpSecurity: 'tls' | 'start_tls'
  smtpUsername: string
  senderAddress: string
  senderName: string
  recipients: string[]
}

export interface AlertSettingsView extends AlertSettingsValues {
  revision: number
  passwordConfigured: boolean
  updatedAt: string
  updatedAtDisplay: string
}

export interface UpdateAlertSettingsPayload extends AlertSettingsValues {
  revision: number
  smtpPassword?: string
  clearPassword: boolean
}

export interface AlertSample {
  windowStart: string
  windowEnd: string
  windowStartDisplay: string
  windowEndDisplay: string
  requests: number
  failures: number
  failureRatePercent: number | null
}

export type AlertPhase = 'normal' | 'pending' | 'firing'
export type AlertEvaluationStatus = 'disabled' | 'waiting' | 'ready' | 'insufficient_samples' | 'unavailable'
export type AlertNotificationKind = 'triggered' | 'reminder' | 'recovered' | 'test'

export interface AlertStatus {
  runtime: {
    phase: AlertPhase
    evaluationStatus: AlertEvaluationStatus
    abnormalCount: number
    recoveryCount: number
    checkedAtDisplay: string | null
    stale: boolean
    sample: AlertSample | null
  }
  events: {
    id: string
    status: 'firing' | 'recovered' | 'cancelled'
    openedAtDisplay: string
    closedAtDisplay: string | null
    sample: AlertSample
  }[]
  deliveries: {
    id: string
    kind: AlertNotificationKind
    recipient: string
    status: 'pending' | 'sending' | 'sent' | 'failed' | 'cancelled'
    attempts: number
    createdAtDisplay: string
    sentAtDisplay: string | null
    nextAttemptAtDisplay: string
    errorMessage: string | null
  }[]
}

export function getAlertSettings(options: RequestOptions = {}) {
  return request<AlertSettingsView>({
    url: '/api/admin/settings/alerts',
    method: 'GET',
    ...options,
  })
}

export function updateAlertSettings(data: UpdateAlertSettingsPayload) {
  return request<AlertSettingsView>({
    url: '/api/admin/settings/alerts/update',
    method: 'POST',
    data,
  })
}

export function getAlertStatus(options: RequestOptions = {}) {
  return request<AlertStatus>({
    url: '/api/admin/settings/alerts/status',
    method: 'GET',
    ...options,
  })
}

export function previewAlertWindow(options: RequestOptions = {}) {
  return request<AlertSample>({
    url: '/api/admin/settings/alerts/preview',
    method: 'GET',
    ...options,
  })
}

export function sendAlertTestEmail(revision: number) {
  return request<string>({
    url: '/api/admin/settings/alerts/test',
    method: 'POST',
    data: { revision },
  })
}
