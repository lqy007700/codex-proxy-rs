import type { AlertSample, AlertSettingsView, AlertStatus, QuotaWarningPreview, UpdateAlertSettingsPayload } from '@/api'

import { isEqual } from 'es-toolkit'
import { computed, onScopeDispose, ref, shallowRef } from 'vue'
import { getAlertSettings, getAlertStatus, previewAlertWindow, previewQuotaWarnings, sendAlertTestEmail, updateAlertSettings } from '@/api'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { useRequestState } from '@/composables/useRequestState'
import { notification } from '@/utils/notification'

export interface AlertSettingsForm {
  enabled: boolean
  quotaWarningEnabled: boolean
  shortTermThresholdPercent: string
  weeklyThresholdPercent: string
  windowMinutes: string
  minimumRequests: string
  failureThresholdPercent: string
  consecutiveEvaluations: string
  recoveryThresholdPercent: string
  reminderEnabled: boolean
  reminderMinutes: string
  recoveryNotification: boolean
  smtpHost: string
  smtpPort: string
  smtpSecurity: 'tls' | 'start_tls'
  smtpUsername: string
  smtpPassword: string
  clearPassword: boolean
  senderAddress: string
  senderName: string
  recipients: string
}

function toForm(data: AlertSettingsView): AlertSettingsForm {
  return {
    enabled: data.enabled,
    quotaWarningEnabled: data.quotaWarning?.enabled ?? false,
    shortTermThresholdPercent: String(data.quotaWarning?.shortTermThresholdPercent ?? 80),
    weeklyThresholdPercent: String(data.quotaWarning?.weeklyThresholdPercent ?? 80),
    windowMinutes: String(data.windowMinutes),
    minimumRequests: String(data.minimumRequests),
    failureThresholdPercent: String(data.failureThresholdPercent),
    consecutiveEvaluations: String(data.consecutiveEvaluations),
    recoveryThresholdPercent: String(data.recoveryThresholdPercent),
    reminderEnabled: data.reminderMinutes !== null,
    reminderMinutes: String(data.reminderMinutes ?? 30),
    recoveryNotification: data.recoveryNotification,
    smtpHost: data.smtpHost,
    smtpPort: String(data.smtpPort),
    smtpSecurity: data.smtpSecurity,
    smtpUsername: data.smtpUsername,
    smtpPassword: '',
    clearPassword: false,
    senderAddress: data.senderAddress,
    senderName: data.senderName,
    recipients: data.recipients.join(', '),
  }
}

export function useAlertSettings() {
  const saved = shallowRef<AlertSettingsView | null>(null)
  const form = ref<AlertSettingsForm | null>(null)
  const savedForm = shallowRef<AlertSettingsForm | null>(null)
  const status = shallowRef<AlertStatus | null>(null)
  const preview = shallowRef<AlertSample | null>(null)
  const quotaPreview = shallowRef<QuotaWarningPreview | null>(null)
  const settingsRequest = useRequestState()
  const statusRequest = useRequestState()
  const previewRequest = useRequestState()
  const saveAction = useAsyncAction()
  const testAction = useAsyncAction()
  const previewAction = useAsyncAction()
  const hasChanges = computed(() => !!form.value && !isEqual(form.value, savedForm.value))
  const busy = computed(() => settingsRequest.loading.value || saveAction.loading.value || testAction.loading.value || previewAction.loading.value)
  let active = false
  let timer: ReturnType<typeof setTimeout> | undefined

  function apply(data: AlertSettingsView) {
    saved.value = data
    form.value = toForm(data)
    savedForm.value = toForm(data)
    preview.value = null
    quotaPreview.value = null
  }

  async function load() {
    if (saved.value || settingsRequest.loading.value)
      return
    const id = settingsRequest.start()
    try {
      const data = await getAlertSettings({ signal: settingsRequest.signal })
      if (settingsRequest.isCurrent(id))
        apply(data)
    }
    catch (cause) {
      settingsRequest.fail(id, cause)
    }
    finally {
      settingsRequest.finish(id)
    }
  }

  async function refreshStatus() {
    if (statusRequest.loading.value || busy.value || !active || !saved.value)
      return
    const id = statusRequest.start()
    try {
      const data = await getAlertStatus({ signal: statusRequest.signal, silent: true, retry: false })
      if (statusRequest.isCurrent(id))
        status.value = data
    }
    catch (cause) {
      statusRequest.fail(id, cause)
    }
    finally {
      statusRequest.finish(id)
    }
  }

  function payload(): UpdateAlertSettingsPayload {
    const draft = form.value
    const current = saved.value
    if (!draft || !current)
      throw new Error('请先加载告警配置')
    function integer(text: string, label: string, min: number, max: number) {
      const value = Number(text)
      if (!text.trim() || !Number.isInteger(value) || value < min || value > max)
        throw new Error(`${label}应为 ${min} 至 ${max} 的整数`)
      return value
    }
    const trigger = integer(draft.failureThresholdPercent, '触发阈值', 2, 100)
    const recovery = integer(draft.recoveryThresholdPercent, '恢复阈值', 1, trigger - 1)
    return {
      revision: current.revision,
      enabled: draft.enabled,
      quotaWarning: {
        enabled: draft.quotaWarningEnabled,
        shortTermThresholdPercent: integer(draft.shortTermThresholdPercent, '5 小时额度使用阈值', 1, 100),
        weeklyThresholdPercent: integer(draft.weeklyThresholdPercent, '周额度使用阈值', 1, 100),
      },
      windowMinutes: integer(draft.windowMinutes, '统计周期', 1, 60),
      minimumRequests: integer(draft.minimumRequests, '最小请求数', 1, 1000000),
      failureThresholdPercent: trigger,
      consecutiveEvaluations: integer(draft.consecutiveEvaluations, '连续触发次数', 1, 10),
      recoveryThresholdPercent: recovery,
      reminderMinutes: draft.reminderEnabled
        ? integer(draft.reminderMinutes, '提醒间隔', 5, 1440)
        : null,
      recoveryNotification: draft.recoveryNotification,
      smtpHost: draft.smtpHost.trim(),
      smtpPort: integer(draft.smtpPort, 'SMTP 端口', 1, 65535),
      smtpSecurity: draft.smtpSecurity,
      smtpUsername: draft.smtpUsername.trim(),
      smtpPassword: draft.smtpPassword || undefined,
      clearPassword: draft.clearPassword,
      senderAddress: draft.senderAddress.trim(),
      senderName: draft.senderName.trim(),
      recipients: [
        ...new Set(draft.recipients.split(/[,，\n]+/).map(value => value.trim()).filter(Boolean)),
      ],
    }
  }

  async function save() {
    if (busy.value || !hasChanges.value)
      return
    statusRequest.invalidate()
    await saveAction.run(async () => {
      const data = await updateAlertSettings(payload())
      apply(data)
      notification.success({ message: '告警配置已保存' })
    })
    await refreshStatus()
  }

  function reset() {
    if (saved.value && !busy.value)
      apply(saved.value)
  }

  async function testEmail() {
    if (busy.value || hasChanges.value || !saved.value)
      return
    statusRequest.invalidate()
    await testAction.run(async () => {
      await sendAlertTestEmail(saved.value!.revision)
      notification.success({ message: '测试邮件已排队，请查看投递结果' })
    })
    await refreshStatus()
  }

  async function previewWindow() {
    if (busy.value || hasChanges.value || !saved.value)
      return
    statusRequest.invalidate()
    const id = previewRequest.start()
    await previewAction.run(async () => {
      const sample = await previewAlertWindow({ retry: false, signal: previewRequest.signal })
      if (previewRequest.isCurrent(id))
        preview.value = sample
    })
    previewRequest.finish(id)
  }

  async function previewQuota() {
    if (busy.value || hasChanges.value || !saved.value)
      return
    const id = previewRequest.start()
    await previewAction.run(async () => {
      const result = await previewQuotaWarnings({ retry: false, signal: previewRequest.signal })
      if (previewRequest.isCurrent(id))
        quotaPreview.value = result
    })
    previewRequest.finish(id)
  }

  function stop() {
    active = false
    clearTimeout(timer)
    timer = undefined
    settingsRequest.invalidate()
    statusRequest.invalidate()
    previewRequest.invalidate()
  }

  function schedule() {
    clearTimeout(timer)
    if (!active)
      return
    timer = setTimeout(async () => {
      if (!document.hidden)
        await refreshStatus()
      schedule()
    }, 10000)
  }

  async function start() {
    active = true
    await load()
    if (!active)
      return
    await refreshStatus()
    schedule()
  }

  onScopeDispose(() => {
    stop()
    if (form.value)
      form.value.smtpPassword = ''
  })

  return {
    form,
    saved,
    status,
    preview,
    quotaPreview,
    hasChanges,
    busy,
    loading: settingsRequest.loading,
    error: settingsRequest.error,
    statusLoading: statusRequest.loading,
    statusError: statusRequest.error,
    saving: saveAction.loading,
    testing: testAction.loading,
    previewing: previewAction.loading,
    start,
    stop,
    load,
    save,
    reset,
    refreshStatus,
    testEmail,
    previewWindow,
    previewQuota,
  }
}
