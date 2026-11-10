import type { DashboardHealthStatus, DashboardHealthTimelinePoint } from '@/api'
import { formatInteger } from '@/utils/format'

interface DashboardHealthStatusMeta {
  label: string
  cellClass: string
  badgeClass: string
  valueClass: string
}

export const healthLegend = [
  { status: 'no_data', label: '无有效样本' },
  { status: 'unavailable', label: '不可达' },
  { status: 'unstable', label: '不稳定' },
  { status: 'low_sample', label: '低样本' },
  { status: 'stable', label: '稳定' },
] satisfies { status: DashboardHealthStatus, label: string }[]

export const healthStatusMeta: Record<DashboardHealthStatus, DashboardHealthStatusMeta> = {
  future: {
    label: '未来',
    cellClass: 'bg-z-bg-container-disabled opacity-60',
    badgeClass: 'bg-z-fill-tertiary text-z-text-quaternary',
    valueClass: 'text-z-text-quaternary',
  },
  no_data: {
    label: '无有效样本',
    cellClass: 'bg-z-border',
    badgeClass: 'bg-z-fill-tertiary text-z-text-secondary',
    valueClass: 'text-z-text-quaternary',
  },
  unavailable: {
    label: '不可达',
    cellClass: 'bg-z-error',
    badgeClass: 'bg-z-error-container text-z-error-on-container',
    valueClass: 'text-z-error-text',
  },
  unstable: {
    label: '不稳定',
    cellClass: 'bg-z-warning',
    badgeClass: 'bg-z-warning-container text-z-warning-on-container',
    valueClass: 'text-z-warning-text',
  },
  low_sample: {
    label: '低样本',
    cellClass: 'bg-z-cyan-solid',
    badgeClass: 'bg-z-cyan-container text-z-cyan-on-container',
    valueClass: 'text-z-cyan-text',
  },
  stable: {
    label: '稳定',
    cellClass: 'bg-z-success',
    badgeClass: 'bg-z-success-container text-z-success-on-container',
    valueClass: 'text-z-success-text',
  },
}

export function healthServiceFailureCount(point: Pick<DashboardHealthTimelinePoint, 'failedRequests' | 'incompleteRequests'>) {
  return point.failedRequests + point.incompleteRequests
}

export function healthEligibleRequestCount(point: Pick<DashboardHealthTimelinePoint, 'successRequests' | 'failedRequests' | 'incompleteRequests'>) {
  return point.successRequests + healthServiceFailureCount(point)
}

export function healthObservedRequestCount(point: DashboardHealthTimelinePoint) {
  return healthEligibleRequestCount(point) + point.cancelledRequests + point.callerErrorRequests
}

export function formatHealthCount(value: number) {
  return formatInteger(Math.max(0, value))
}
