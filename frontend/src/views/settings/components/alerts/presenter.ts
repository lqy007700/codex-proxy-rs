import type { AlertStatus } from '@/api'

export function rateLabel(value: number | null | undefined): string {
  return value === null || value === undefined
    ? '—'
    : `${value.toFixed(1)}%`
}

export function phaseLabel(runtime: AlertStatus['runtime'] | undefined): string {
  if (!runtime)
    return '尚未读取'
  if (runtime.evaluationStatus === 'disabled')
    return '未启用'
  if (runtime.phase === 'normal' && (runtime.stale || runtime.evaluationStatus !== 'ready'))
    return '尚未判定'
  return { normal: '正常', pending: '待触发', firing: '告警中' }[runtime.phase]
}

export function evaluationLabel(runtime: AlertStatus['runtime'] | undefined): string {
  if (!runtime)
    return '尚未读取'
  if (runtime.stale)
    return '检查结果已过期'
  return {
    disabled: '未启用',
    waiting: '等待首次检查',
    ready: '检查完成',
    insufficient_samples: '样本不足',
    unavailable: '检查失败',
  }[runtime.evaluationStatus]
}

export const notificationLabels = {
  triggered: '异常告警',
  reminder: '持续提醒',
  recovered: '恢复通知',
  test: '测试邮件',
  quota_warning: '额度预警',
} as const

export const deliveryLabels = {
  pending: '待发送',
  sending: '发送中',
  sent: '已发送',
  failed: '发送失败',
  cancelled: '已取消',
} as const

export const eventLabels = {
  firing: '告警中',
  recovered: '已恢复',
  cancelled: '配置变更结束',
} as const
