import type { SystemUpdateDetail, SystemVersion } from '@/api'
import { ArrowUpCircle, CheckCircle2, RefreshCw, XCircle } from '@lucide/vue'

interface SystemUpdatePresentationInput {
  version: SystemVersion | null
  updateInfo: SystemUpdateDetail | null
  loading: boolean
  checking: boolean
  restarting: boolean
  updating: boolean
  updateError: string
  needRestart: boolean
  hasUpdate: boolean
  updateStreaming: boolean
  updateStreamError: string
  previousTargetVersion: string
  confirmedTargetVersion: string | null
}

export function resolveSystemUpdatePresentation(input: SystemUpdatePresentationInput) {
  return {
    status: resolveStatus(input),
    summaryItems: resolveSummaryItems(input),
    confirmRows: resolveConfirmRows(input),
    releaseVersion: displayValue(input.updateInfo?.latestVersion),
    streamStatusLabel: input.updateStreaming
      ? '实时'
      : input.updateStreamError ? '断开' : '待连接',
    restartButtonLabel: input.restarting ? '重启中' : '立即重启',
  }
}

export function resolveSystemUpdateLogClasses(level: string) {
  switch (level) {
    case 'success':
      return { marker: 'text-z-success', text: 'text-z-success' }
    case 'warning':
      return { marker: 'text-z-warning', text: 'text-z-warning' }
    case 'error':
      return { marker: 'text-z-error', text: 'text-z-error' }
    default:
      return { marker: 'text-z-info', text: 'text-z-text' }
  }
}

function resolveStatus(input: SystemUpdatePresentationInput) {
  if (input.restarting || input.updating || input.checking || input.loading) {
    return {
      label: input.restarting
        ? '重启中'
        : input.updating ? '更新中' : '检查中',
      icon: RefreshCw,
      badge: 'bg-z-info-container text-z-info-on-container',
      iconClass: 'text-z-info',
    }
  }
  if (input.updateError || input.updateInfo?.warning) {
    return {
      label: '异常',
      icon: XCircle,
      badge: 'bg-z-error-container text-z-error-on-container',
      iconClass: 'text-z-error',
    }
  }
  if (input.needRestart || input.hasUpdate || input.updateInfo) {
    return {
      label: input.needRestart
        ? '待重启'
        : input.hasUpdate ? '有可用更新' : '无可用更新',
      icon: input.hasUpdate ? ArrowUpCircle : CheckCircle2,
      badge: 'bg-z-success-container text-z-success-on-container',
      iconClass: 'text-z-success',
    }
  }
  return {
    label: '未检查',
    icon: CheckCircle2,
    badge: 'bg-z-fill-tertiary text-z-text-secondary',
    iconClass: 'text-z-text-quaternary',
  }
}

function resolveSummaryItems(input: SystemUpdatePresentationInput) {
  return [
    {
      key: 'current',
      label: '当前版本',
      value: input.loading
        ? '...'
        : versionLabel(input.updateInfo?.currentVersion ?? input.version?.version),
      title: input.updateInfo?.currentVersion ?? input.version?.version,
    },
    {
      key: 'latest',
      label: '通道最新',
      value: versionLabel(input.updateInfo?.latestVersion),
      title: input.updateInfo?.latestVersion,
      releaseUrl: input.updateInfo?.releaseUrl,
    },
    {
      key: 'build',
      label: '构建',
      value: displayValue(input.updateInfo?.buildTypeLabel),
      title: input.updateInfo?.buildType,
    },
    {
      key: 'deployment',
      label: '部署',
      value: displayValue(input.version?.deploymentModeLabel),
      title: input.version?.deploymentMode,
    },
  ]
}

function resolveConfirmRows(input: SystemUpdatePresentationInput) {
  return [
    {
      key: 'current',
      label: '当前版本',
      value: versionLabel(input.version?.version),
    },
    {
      key: 'previous',
      label: '已显示目标',
      value: versionLabel(input.previousTargetVersion),
    },
    {
      key: 'target',
      label: '远端最新目标',
      value: versionLabel(input.confirmedTargetVersion),
    },
  ]
}

function versionLabel(value?: string | null) {
  return value
    ? `v${value}`
    : '-'
}

function displayValue(value: unknown) {
  return value
    ? String(value)
    : '-'
}
