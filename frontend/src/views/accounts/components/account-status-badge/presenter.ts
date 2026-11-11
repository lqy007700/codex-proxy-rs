import type { Component } from 'vue'
import type { AccountErrorReason, AccountProtectionRule, AccountStatus } from '@/api'
import { AlertTriangle, CircleCheck, Gauge, Power, Timer } from '@lucide/vue'

import { errorReasonLabels, statusLabels, statusTones } from '../../constants'

type AccountStatusDisplayMode = AccountStatus | 'refresh_backoff'

type AccountStatusTone = (typeof statusTones)[AccountStatus]

interface AccountStatusDisplayDefinition {
  tone: AccountStatusTone
  label: string
  title?: string
  description: string
  recoveryHint: string
  icon: Component
}

interface AccountStatusStyle {
  text: string
  dot: string
  badge: string
  icon: string
}

interface AccountStatusPresentation {
  mode: AccountStatusDisplayMode
  statusStyle: AccountStatusStyle
  label: string
  title: string
  description: string
  recoveryHint: string
  icon: Component
  hasDetail: boolean
  errorText: string | null
  nextRefreshDisplay: string | null
  rateLimitRecovery: string | null
  recoveryTimeLabel: string
  protectionRules: AccountProtectionRule[]
  triggerLabel: string
}

interface AccountStatusPresentationInput {
  status: AccountStatus
  errorReason: AccountErrorReason | null
  errorMessage: string | null
  rateLimitRecoveryDisplay: string | null
  rateLimitReason: 'upstream_rate_limit' | 'error_protection' | null
  protectionRules: AccountProtectionRule[]
  recoveryProbeRequired: boolean
  nextRefreshAt: string | null
  nextRefreshAtDisplay: string | null
  now: number
}

const statusStyles: Record<AccountStatusTone, AccountStatusStyle> = {
  success: {
    text: 'text-z-success-text',
    dot: 'bg-z-success',
    badge: 'bg-z-success-container text-z-success-on-container',
    icon: 'bg-z-success-container text-z-success-on-container',
  },
  danger: {
    text: 'text-z-error-text',
    dot: 'bg-z-error',
    badge: 'bg-z-error-container text-z-error-on-container',
    icon: 'bg-z-error-container text-z-error-on-container',
  },
  warning: {
    text: 'text-z-warning-text',
    dot: 'bg-z-warning',
    badge: 'bg-z-warning-container text-z-warning-on-container',
    icon: 'bg-z-warning-container text-z-warning-on-container',
  },
  info: {
    text: 'text-z-info-text',
    dot: 'bg-z-info',
    badge: 'bg-z-info-container text-z-info-on-container',
    icon: 'bg-z-info-container text-z-info-on-container',
  },
  normal: {
    text: 'text-z-text-secondary',
    dot: 'bg-z-text-quaternary',
    badge: 'bg-z-fill-quaternary text-z-text-secondary',
    icon: 'bg-z-fill-quaternary text-z-text-secondary',
  },
}

const displayDefinitions: Record<AccountStatusDisplayMode, AccountStatusDisplayDefinition> = {
  refresh_backoff: {
    tone: 'warning',
    label: '退避中',
    title: 'OAuth 刷新退避',
    description: '刷新失败，系统正在等待下一次自动尝试',
    recoveryHint: '系统会在计划时间自动重试',
    icon: Timer,
  },
  normal: {
    tone: statusTones.normal,
    label: statusLabels.normal,
    description: '该账号当前状态需要关注',
    recoveryHint: '重新测试连接以获取最新状态',
    icon: CircleCheck,
  },
  quota_paused: {
    tone: statusTones.quota_paused,
    label: statusLabels.quota_paused,
    description: '通用额度窗口已达到设置的使用率上限，暂停接收新请求',
    recoveryHint: '使用率回落后恢复调度，也可调整或清除额度上限',
    icon: Gauge,
  },
  quota_exhausted: {
    tone: statusTones.quota_exhausted,
    label: statusLabels.quota_exhausted,
    description: '该账号当前状态需要关注',
    recoveryHint: '重新测试连接以获取最新状态',
    icon: Gauge,
  },
  rate_limited: {
    tone: statusTones.rate_limited,
    label: statusLabels.rate_limited,
    description: '上游暂时限制了请求频率，系统正在冷却该账号',
    recoveryHint: '冷却结束后，系统会自动恢复调度',
    icon: Timer,
  },
  disabled: {
    tone: statusTones.disabled,
    label: statusLabels.disabled,
    description: '该账号当前状态需要关注',
    recoveryHint: '重新测试连接以获取最新状态',
    icon: Power,
  },
  error: {
    tone: statusTones.error,
    label: statusLabels.error,
    description: '该账号暂不参与调度，处理凭据后可重新测试连接',
    recoveryHint: '重新测试连接以获取最新状态',
    icon: AlertTriangle,
  },
}

const errorRecoveryHints: Record<AccountErrorReason, string> = {
  account_unverified: '重新授权后，账号会重新参与调度',
  access_token_expired: '重新授权后，账号会重新参与调度',
  credential_expired: '重新授权后，账号会重新参与调度',
  credential_invalid: '请更新或重新导入凭据，再次测试连接',
  account_banned: '请确认上游账号状态，解除限制后再启用',
}

export function resolveAccountStatusPresentation(
  input: AccountStatusPresentationInput,
): AccountStatusPresentation {
  const isBackoff = input.status !== 'disabled'
    && input.nextRefreshAt !== null
    && Date.parse(input.nextRefreshAt) > input.now
  const mode: AccountStatusDisplayMode = isBackoff ? 'refresh_backoff' : input.status
  const isProtected = mode === 'rate_limited' && input.rateLimitReason === 'error_protection'
  const waitsForProbe = isProtected && input.recoveryProbeRequired
  const definition = isProtected
    ? {
        ...displayDefinitions.rate_limited,
        label: '错误保护',
        description: '匹配规则的失败累计达到阈值，系统已暂停该账号的调度',
        recoveryHint: waitsForProbe
          ? '冷却结束后进行恢复探测，成功后恢复调度，也可手动恢复账号'
          : '冷却结束后自动恢复调度，也可手动恢复账号',
      }
    : displayDefinitions[mode]
  const nextRefreshDisplay = isBackoff ? input.nextRefreshAtDisplay : null
  const reasonLabel = input.errorReason ? errorReasonLabels[input.errorReason] : null
  const title = definition.title ?? reasonLabel ?? definition.label
  const rateLimitRecovery = mode === 'rate_limited'
    ? input.rateLimitRecoveryDisplay
    : null
  const recoveryHint = mode !== 'refresh_backoff'
    && mode !== 'rate_limited'
    && input.errorReason
    ? errorRecoveryHints[input.errorReason]
    : definition.recoveryHint
  const hasDetail = input.status === 'error' || mode === 'quota_paused' || mode === 'rate_limited' || isBackoff
  const retry = nextRefreshDisplay
    ? `，下次尝试：${nextRefreshDisplay}`
    : ''
  const triggerLabel = `${title}，${definition.description}${retry}，点击或聚焦查看详情`

  return {
    mode,
    statusStyle: statusStyles[definition.tone],
    label: definition.label,
    title,
    description: definition.description,
    recoveryHint,
    icon: definition.icon,
    hasDetail,
    errorText: input.errorMessage || null,
    nextRefreshDisplay,
    rateLimitRecovery,
    recoveryTimeLabel: waitsForProbe ? '恢复探测' : '预计恢复',
    protectionRules: isProtected
      ? input.protectionRules
      : [],
    triggerLabel,
  }
}
