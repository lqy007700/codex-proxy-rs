import type { ProtectionDraft } from '../components/error-protection/form'
import type { rotationOptions } from '../constants'
import type { AccountAffinity, CodexPrivacyPolicy, SmartSchedulingConfig } from '@/api'
import type { ProviderRequestProfiles, ProviderRequestProfileUpdates } from '@/api/modules/settings/profiles'
import { cloneDeep, isEqual } from 'es-toolkit'
import { computed, onScopeDispose, reactive, ref, shallowRef } from 'vue'

import { detectRequestLocation, getSettings, updateSettings } from '@/api'
import { ApiError } from '@/api/request'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { normalizeRequestLocation, requestLocationError } from '@/utils/location'
import { notification as ZNotification } from '@/utils/notification'
import { errorMessage } from '@/utils/operation'
import { protectionSettingsError, trimProtectionText } from '../components/error-protection/form'

type RotationStrategy = (typeof rotationOptions)[number]['value']

const MIB = 1024 * 1024

export function useSettingsForm() {
  const loading = shallowRef(true)
  const saveAction = useAsyncAction()
  const saving = saveAction.loading
  const locationAction = useAsyncAction()
  const detectingLocation = locationAction.loading
  let locationController: AbortController | undefined
  onScopeDispose(() => locationController?.abort())
  const error = shallowRef('')
  const mappings = ref<Array<{ requestedModel: string, upstreamModel: string }>>([])
  const smartSchedulingDefaults = shallowRef<SmartSchedulingConfig>()
  const form = reactive({
    configRevision: 0,
    smartScheduling: undefined as SmartSchedulingConfig | undefined,
    providerRequestProfiles: {} as ProviderRequestProfiles,
    codexPrivacyPolicy: { enabled: false, onError: 'skip_rule', rules: [] } as CodexPrivacyPolicy,
    requestLocationEnabled: false,
    requestLocation: {
      country: '',
      region: '',
      city: '',
      timezone: '',
    },
    refreshMarginSeconds: null as number | null,
    refreshConcurrency: null as number | null,
    maxConcurrentPerAccount: null as number | null,
    openaiGuardianReservedConcurrency: null as number | null,
    openaiAccountAffinity: '' as AccountAffinity | '',
    maxAccountRotations: null as number | null,
    recoveryAttempts: null as number | null,
    recoveryRotationAttempts: null as number | null,
    openaiSessionAffinityTtlHours: null as number | null,
    maxWaitingPerKey: null as number | null,
    maxWaitingPerAccount: null as number | null,
    concurrencyWaitTimeoutSeconds: null as number | null,
    maxRetryAfterSeconds: null as number | null,
    responsesMaxDecompressedBodyMiB: null as number | null,

    rotationStrategy: '' as RotationStrategy | '',
    minCodexDesktopVersion: '',
    minCodexCliVersion: '',
    usageRetentionDays: 31 as number | null,
    opsEventRetentionDays: 30,
    auditRetentionDays: 90,

    accountErrorProtection: {
      enabled: false,
      rules: [],
      threshold: 12,
      windowSeconds: 600,
      durationSeconds: 7200,
      probeEnabled: true,
      probeModel: '',
    } as ProtectionDraft,
    accountWarmupEnabled: false,
    accountWarmupCron: [] as string[],
    accountWarmupModel: '',
    resetCreditLeadMinutes: 10 as number | null,
    resetCreditMaxWeeklyRemainingPercent: null as number | null,
  })

  function snapshot() {
    return cloneDeep({ form, mappings: mappings.value })
  }

  const saved = shallowRef<ReturnType<typeof snapshot>>()
  const hasChanges = computed(() => saved.value !== undefined
    && !isEqual({ form, mappings: mappings.value }, saved.value))

  function resetSettings() {
    if (!saved.value || saving.value || detectingLocation.value)
      return
    const initial = cloneDeep(saved.value)
    Object.assign(form, initial.form)
    mappings.value = initial.mappings
  }

  function numericModel(key: 'resetCreditMaxWeeklyRemainingPercent' | 'resetCreditLeadMinutes' | 'refreshMarginSeconds' | 'refreshConcurrency' | 'maxConcurrentPerAccount' | 'openaiGuardianReservedConcurrency' | 'maxAccountRotations' | 'recoveryAttempts' | 'recoveryRotationAttempts' | 'openaiSessionAffinityTtlHours' | 'maxWaitingPerKey' | 'maxWaitingPerAccount' | 'concurrencyWaitTimeoutSeconds' | 'maxRetryAfterSeconds' | 'responsesMaxDecompressedBodyMiB' | 'usageRetentionDays') {
    return computed({
      get: () => (form[key] === null
        ? ''
        : String(form[key])),
      set: (value: string) => {
        if (!value.trim()) {
          form[key] = null
          return
        }
        const parsed = Number(value)
        form[key] = Number.isFinite(parsed)
          ? parsed
          : null
      },
    })
  }

  const refreshMarginSecondsValue = numericModel('refreshMarginSeconds')
  const refreshConcurrencyValue = numericModel('refreshConcurrency')
  const maxConcurrentPerAccountValue = numericModel('maxConcurrentPerAccount')
  const openaiGuardianReservedConcurrencyValue = numericModel('openaiGuardianReservedConcurrency')
  const maxAccountRotationsValue = numericModel('maxAccountRotations')
  const recoveryAttemptsValue = numericModel('recoveryAttempts')
  const recoveryRotationAttemptsValue = numericModel('recoveryRotationAttempts')
  const openaiSessionAffinityTtlHoursValue = numericModel('openaiSessionAffinityTtlHours')
  const maxWaitingPerKeyValue = numericModel('maxWaitingPerKey')
  const maxWaitingPerAccountValue = numericModel('maxWaitingPerAccount')
  const responsesMaxDecompressedBodyMiBValue = numericModel('responsesMaxDecompressedBodyMiB')
  const resetCreditLeadMinutesValue = numericModel('resetCreditLeadMinutes')
  const resetCreditMaxWeeklyRemainingPercentValue = numericModel('resetCreditMaxWeeklyRemainingPercent')
  const usageRetentionDaysValue = numericModel('usageRetentionDays')
  const concurrencyWaitTimeoutSecondsValue = numericModel('concurrencyWaitTimeoutSeconds')
  const maxRetryAfterSecondsValue = numericModel('maxRetryAfterSeconds')

  const minCodexDesktopVersionError = computed(() => versionError(form.minCodexDesktopVersion))
  const minCodexCliVersionError = computed(() => versionError(form.minCodexCliVersion))

  function versionError(value: string): string {
    const normalized = value.trim()
    return normalized && !isSemver(normalized)
      ? '请输入标准 SemVer，例如 0.152.0'
      : ''
  }

  function applySettings(data: Awaited<ReturnType<typeof getSettings>>) {
    form.codexPrivacyPolicy = cloneDeep(data.codexPrivacyPolicy)
    form.configRevision = data.configRevision
    form.requestLocationEnabled = data.requestLocationEnabled
    form.requestLocation = { ...data.requestLocation }
    form.refreshMarginSeconds = data.refreshMarginSeconds
    form.refreshConcurrency = data.refreshConcurrency
    form.maxConcurrentPerAccount = data.maxConcurrentPerAccount
    form.openaiGuardianReservedConcurrency = data.openaiGuardianReservedConcurrency
    form.openaiAccountAffinity = data.openaiAccountAffinity
    form.maxAccountRotations = data.maxAccountRotations
    form.recoveryAttempts = data.recoveryAttempts
    form.recoveryRotationAttempts = data.recoveryRotationAttempts
    form.openaiSessionAffinityTtlHours = data.openaiSessionAffinityTtlHours
    form.maxWaitingPerKey = data.maxWaitingPerKey
    form.maxWaitingPerAccount = data.maxWaitingPerAccount
    form.concurrencyWaitTimeoutSeconds = data.concurrencyWaitTimeoutSeconds
    form.maxRetryAfterSeconds = data.maxRetryAfterSeconds
    form.responsesMaxDecompressedBodyMiB = data.responsesMaxDecompressedBodyBytes / MIB

    form.smartScheduling = { ...data.smartScheduling }
    smartSchedulingDefaults.value = { ...data.smartSchedulingDefaults }
    form.rotationStrategy = data.rotationStrategy
    form.minCodexDesktopVersion = data.minCodexDesktopVersion ?? ''
    form.providerRequestProfiles = cloneDeep(data.providerRequestProfiles)
    form.minCodexCliVersion = data.minCodexCliVersion ?? ''
    form.usageRetentionDays = data.usageRetentionDays
    form.opsEventRetentionDays = data.opsEventRetentionDays
    form.auditRetentionDays = data.auditRetentionDays
    form.accountErrorProtection = {
      ...cloneDeep(data.accountErrorProtection),
      probeModel: data.accountErrorProtection.probeModel ?? '',
    }
    form.accountWarmupEnabled = data.accountWarmupEnabled
    form.accountWarmupCron = data.accountWarmupCron
      ? data.accountWarmupCron.split(';')
      : []
    form.accountWarmupModel = data.accountWarmupModel ?? ''
    form.resetCreditLeadMinutes = data.resetCreditLeadMinutes
    form.resetCreditMaxWeeklyRemainingPercent = data.resetCreditMaxWeeklyRemainingPercent
    mappings.value = Object.entries(data.modelMappings).map(([requestedModel, upstreamModel]) => ({
      requestedModel,
      upstreamModel,
    }))
    saved.value = snapshot()
  }

  async function loadSettings(silent = false) {
    if (detectingLocation.value)
      return
    loading.value = true
    error.value = ''
    try {
      applySettings(await getSettings({ silent }))
    }
    catch (cause: unknown) {
      error.value = errorMessage(cause)
    }
    finally {
      loading.value = false
    }
  }

  async function detectLocation() {
    if (saving.value || loading.value || error.value)
      return
    await locationAction.run(async () => {
      const controller = new AbortController()
      locationController = controller
      const result = await detectRequestLocation({ signal: controller.signal })
      if (controller.signal.aborted)
        return
      if (result.status === 'detected') {
        form.requestLocation = { ...result.location }
        form.requestLocationEnabled = true
        ZNotification.success({ message: '已填入出口位置' })
      }
      else if (result.status === 'conflict') {
        ZNotification.warning({ message: 'IPv4 与 IPv6 出口时区不一致，请手动填写' })
      }
      else {
        ZNotification.warning({
          message: result.status === 'failed'
            ? result.message
            : '未获取到出口位置',
        })
      }
    })
  }

  function addMapping() {
    mappings.value = [
      ...mappings.value,
      { requestedModel: '', upstreamModel: '' },
    ]
  }

  function updateMapping(index: number, key: 'requestedModel' | 'upstreamModel', value: string) {
    const rows = [...mappings.value]
    if (!rows[index])
      return
    rows[index] = { ...rows[index], [key]: value }
    mappings.value = rows
  }

  function removeMapping(index: number) {
    const rows = [...mappings.value]
    rows.splice(index, 1)
    mappings.value = rows
  }

  function mappingPayload() {
    const entries: Record<string, string> = {}
    for (const row of mappings.value) {
      const requested = row.requestedModel.trim()
      const upstream = row.upstreamModel.trim()
      if (!requested || !upstream)
        throw new Error('请完整填写模型映射')
      if (entries[requested])
        throw new Error(`存在重复的客户端模型：${requested}`)
      entries[requested] = upstream
    }
    return entries
  }

  async function saveSettings() {
    const smartScheduling = form.smartScheduling
    if (!smartScheduling)
      return
    const savedSettings = saved.value
    if (saving.value || loading.value || detectingLocation.value || !savedSettings)
      return
    const { refreshMarginSeconds, refreshConcurrency, maxConcurrentPerAccount, openaiGuardianReservedConcurrency, openaiAccountAffinity, maxAccountRotations, recoveryAttempts, recoveryRotationAttempts, openaiSessionAffinityTtlHours, rotationStrategy, maxWaitingPerKey, maxWaitingPerAccount, concurrencyWaitTimeoutSeconds, maxRetryAfterSeconds, responsesMaxDecompressedBodyMiB } = form
    if (!openaiAccountAffinity || maxAccountRotations === null || openaiSessionAffinityTtlHours === null || refreshMarginSeconds === null || refreshConcurrency === null || maxConcurrentPerAccount === null || openaiGuardianReservedConcurrency === null || !rotationStrategy || maxWaitingPerKey === null || maxWaitingPerAccount === null || concurrencyWaitTimeoutSeconds === null) {
      ZNotification.warning({ message: '请完整填写并发、队列、凭据刷新参数和调度策略' })
      return
    }
    if (!Number.isInteger(openaiSessionAffinityTtlHours) || openaiSessionAffinityTtlHours < 1 || openaiSessionAffinityTtlHours > 720) {
      ZNotification.warning({ message: '亲和时长应为 1～720 小时的整数' })
      return
    }
    if (!Number.isInteger(maxAccountRotations) || maxAccountRotations < 0 || maxAccountRotations > 31) {
      ZNotification.warning({ message: '其他恢复换号上限应为 0～31 的整数，0 表示禁止' })
      return
    }
    if (maxRetryAfterSeconds === null || !Number.isInteger(maxRetryAfterSeconds) || maxRetryAfterSeconds < 0 || maxRetryAfterSeconds > 4294967295) {
      ZNotification.warning({ message: '上游重试等待上限需为 0～4294967295 的整数' })
      return
    }
    if (recoveryAttempts === null || !Number.isInteger(recoveryAttempts) || recoveryAttempts < 1 || recoveryAttempts > 31) {
      ZNotification.warning({ message: '初始账号尝试次数应为 1～31 的整数，包含首次请求' })
      return
    }
    if (recoveryRotationAttempts === null || !Number.isInteger(recoveryRotationAttempts) || recoveryRotationAttempts < 0 || recoveryRotationAttempts > 30) {
      ZNotification.warning({ message: '换号重试次数需为 0～30 的整数' })
      return
    }
    if (recoveryAttempts + recoveryRotationAttempts > 31) {
      ZNotification.warning({ message: '初始账号尝试与换号重试合计不能超过 31 次' })
      return
    }
    if (!Number.isInteger(maxConcurrentPerAccount) || maxConcurrentPerAccount < 0 || maxConcurrentPerAccount > 4294967295) {
      ZNotification.warning({ message: '默认账号并发上限应为 0～4294967295 的整数，0 表示不限制' })
      return
    }
    if (!Number.isInteger(openaiGuardianReservedConcurrency) || openaiGuardianReservedConcurrency < 0 || openaiGuardianReservedConcurrency > 4294967295) {
      ZNotification.warning({ message: '自动审批独立并发应为 0～4294967295 的整数，0 表示共用普通并发' })
      return
    }
    if (responsesMaxDecompressedBodyMiB === null || !Number.isInteger(responsesMaxDecompressedBodyMiB) || responsesMaxDecompressedBodyMiB < 1
      || !Number.isSafeInteger(responsesMaxDecompressedBodyMiB * MIB)) {
      ZNotification.warning({ message: 'Responses 解压上限应为有效的正整数（MiB）' })
      return
    }
    const resetCreditMaxWeeklyRemainingPercent = form.resetCreditMaxWeeklyRemainingPercent
    if (resetCreditMaxWeeklyRemainingPercent !== null && (!Number.isInteger(resetCreditMaxWeeklyRemainingPercent) || resetCreditMaxWeeklyRemainingPercent < 0 || resetCreditMaxWeeklyRemainingPercent > 100)) {
      ZNotification.warning({ message: '剩余周额度上限应为 0～100 的整数，留空表示不限制' })
      return
    }
    const resetCreditLeadMinutes = form.resetCreditLeadMinutes
    if (resetCreditLeadMinutes === null || !Number.isInteger(resetCreditLeadMinutes) || resetCreditLeadMinutes < 1 || resetCreditLeadMinutes > 1440) {
      ZNotification.warning({ message: '重置卡提前使用时间应为 1～1440 分钟的整数' })
      return
    }
    const usageRetentionDays = form.usageRetentionDays
    if (usageRetentionDays === null || !Number.isInteger(usageRetentionDays) || usageRetentionDays < 1 || usageRetentionDays > 31) {
      ZNotification.warning({ message: '历史保留天数应为 1–31 的整数' })
      return
    }
    if (![maxWaitingPerKey, maxWaitingPerAccount].every(value => Number.isInteger(value) && value >= 0 && value <= 1000)
      || !Number.isInteger(concurrencyWaitTimeoutSeconds) || concurrencyWaitTimeoutSeconds < 1 || concurrencyWaitTimeoutSeconds > 120) {
      ZNotification.warning({ message: '队列容量应为 0～1000 的整数，排队超时应为 1～120 秒的整数' })
      return
    }
    if (minCodexDesktopVersionError.value || minCodexCliVersionError.value) {
      ZNotification.warning({ message: '请修正客户端最低版本格式' })
      return
    }
    // 关闭时保留已保存的自定义值，未完成的草稿不阻止停止覆盖。
    const requestLocation = form.requestLocationEnabled
      ? normalizeRequestLocation(form.requestLocation)
      : savedSettings.form.requestLocation
    const locationError = requestLocationError(requestLocation)
    if (locationError) {
      ZNotification.warning({ message: locationError })
      return
    }
    const protectionError = protectionSettingsError(form.accountErrorProtection)
    if (protectionError) {
      ZNotification.warning({ message: protectionError })
      return
    }
    const cron = form.accountWarmupCron.join(';')
    if ((form.accountWarmupEnabled && form.accountWarmupCron.length === 0)
      || !form.accountWarmupCron.every(expression => /^[^\s;]+(?: +[^\s;]+){4}$/u.test(expression))) {
      ZNotification.warning({ message: '每条规则请输入五段 Cron 表达式，例如 0 8 * * *' })
      return
    }
    if (new TextEncoder().encode(cron).length > 255) {
      ZNotification.warning({ message: '预激活规则总长度不能超过 255 字节' })
      return
    }
    const warmupModel = form.accountWarmupModel.trim()
    if (form.accountWarmupEnabled && !warmupModel) {
      ZNotification.warning({ message: '启用预激活时请选择模型' })
      return
    }
    if (warmupModel.length > 128) {
      ZNotification.warning({ message: '预激活模型名称不能超过 128 个字符' })
      return
    }
    await saveAction.run(async () => {
      const result = await updateSettings({
        configRevision: savedSettings.form.configRevision,
        providerRequestProfiles: requestProfileUpdates(
          savedSettings.form.providerRequestProfiles,
          form.providerRequestProfiles,
        ),
        codexPrivacyPolicy: cloneDeep(form.codexPrivacyPolicy),
        requestLocationEnabled: form.requestLocationEnabled,
        requestLocation,
        modelMappings: mappingPayload(),
        refreshMarginSeconds,
        refreshConcurrency,
        maxConcurrentPerAccount,
        openaiGuardianReservedConcurrency,
        openaiAccountAffinity,
        maxAccountRotations,
        recoveryAttempts,
        recoveryRotationAttempts,
        openaiSessionAffinityTtlHours,
        maxWaitingPerKey,
        maxWaitingPerAccount,
        concurrencyWaitTimeoutSeconds,
        maxRetryAfterSeconds,
        responsesMaxDecompressedBodyBytes: responsesMaxDecompressedBodyMiB * MIB,
        rotationStrategy,
        smartScheduling: { ...smartScheduling },
        minCodexDesktopVersion: form.minCodexDesktopVersion.trim() || null,
        minCodexCliVersion: form.minCodexCliVersion.trim() || null,
        usageRetentionDays,
        opsEventRetentionDays: form.opsEventRetentionDays,
        auditRetentionDays: form.auditRetentionDays,
        accountErrorProtection: {
          ...cloneDeep(form.accountErrorProtection),
          threshold: form.accountErrorProtection.threshold!,
          windowSeconds: form.accountErrorProtection.windowSeconds!,
          durationSeconds: form.accountErrorProtection.durationSeconds!,
          probeModel: trimProtectionText(form.accountErrorProtection.probeModel) || null,
        },
        accountWarmupEnabled: form.accountWarmupEnabled,
        accountWarmupCron: cron,
        accountWarmupModel: warmupModel || null,
        resetCreditLeadMinutes,
        resetCreditMaxWeeklyRemainingPercent,
      })
      applySettings(result)
      ZNotification.success({ message: '设置已保存' })
    }, {
      onError: (cause) => {
        // 明确拒绝的请求保留草稿，执行结果未知时仍回读设置以同步已提交的版本
        if (cause instanceof ApiError && cause.status !== 409
          && (cause.kind !== 'api' || cause.status < 400 || cause.status >= 500)) {
          void loadSettings(true)
        }
      },
    })
  }

  return {
    loading,
    saving,
    detectingLocation,
    detectLocation,
    hasChanges,
    resetSettings,
    error,
    form,
    smartSchedulingDefaults,
    mappings,
    addMapping,
    updateMapping,
    removeMapping,
    refreshMarginSecondsValue,
    refreshConcurrencyValue,
    maxConcurrentPerAccountValue,
    openaiGuardianReservedConcurrencyValue,
    maxAccountRotationsValue,
    recoveryAttemptsValue,
    recoveryRotationAttemptsValue,
    openaiSessionAffinityTtlHoursValue,
    maxWaitingPerKeyValue,
    maxWaitingPerAccountValue,
    concurrencyWaitTimeoutSecondsValue,
    maxRetryAfterSecondsValue,
    responsesMaxDecompressedBodyMiBValue,
    usageRetentionDaysValue,
    resetCreditLeadMinutesValue,
    resetCreditMaxWeeklyRemainingPercentValue,
    minCodexDesktopVersionError,
    minCodexCliVersionError,
    saveSettings,
    loadSettings,
  }
}

function requestProfileUpdates(
  previous: ProviderRequestProfiles,
  current: ProviderRequestProfiles,
): ProviderRequestProfileUpdates {
  const updates: ProviderRequestProfileUpdates = {}
  const clonedCurrent = cloneDeep(current)
  for (const provider of new Set([
    ...Object.keys(previous),
    ...Object.keys(current),
  ])) {
    if (isEqual(previous[provider], current[provider]))
      continue
    updates[provider] = clonedCurrent[provider] ?? null
  }
  return updates
}

function isSemver(value: string): boolean {
  if (value.length > 64 || value.startsWith('v'))
    return false

  const buildParts = value.split('+')
  if (buildParts.length > 2)
    return false
  const [versionAndPrerelease = '', build] = buildParts
  if (build !== undefined && !validIdentifiers(build, false))
    return false

  const prereleaseSeparator = versionAndPrerelease.indexOf('-')
  const core = prereleaseSeparator < 0
    ? versionAndPrerelease
    : versionAndPrerelease.slice(0, prereleaseSeparator)
  const prerelease = prereleaseSeparator < 0
    ? undefined
    : versionAndPrerelease.slice(prereleaseSeparator + 1)
  if (prerelease !== undefined && !validIdentifiers(prerelease, true))
    return false

  const coreParts = core.split('.')
  return coreParts.length === 3 && coreParts.every(validCoreNumericIdentifier)
}

function validIdentifiers(value: string, rejectNumericLeadingZeros: boolean): boolean {
  return Boolean(value) && value.split('.').every((identifier) => {
    if (!identifier || !/^[\da-z-]+$/i.test(identifier))
      return false
    return !rejectNumericLeadingZeros || !/^\d+$/.test(identifier) || validNumericIdentifier(identifier)
  })
}

function validNumericIdentifier(value: string): boolean {
  return /^(?:0|[1-9]\d*)$/.test(value)
}

function validCoreNumericIdentifier(value: string): boolean {
  return validNumericIdentifier(value) && BigInt(value) <= 18_446_744_073_709_551_615n
}
