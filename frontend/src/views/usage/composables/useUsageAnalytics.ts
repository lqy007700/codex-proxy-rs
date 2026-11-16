import type { Ref } from 'vue'
import type { AnalyticsMetadata } from '@/api/modules/analytics'
import type { CallerUsageFilters } from '@/api/modules/usage'
import type { UsageTimeRangeParams } from '@/composables/useUsageTimeRange'
import { onMounted, onScopeDispose, shallowRef, watch } from 'vue'
import { getUsageRecordInsightsDiagnostics, getUsageRecordInsightsOverview, getUsageRecordSummary } from '@/api'
import { withMinimumDuration } from '@/utils/operation'

interface UseUsageAnalyticsOptions {
  timeRangeParams: Readonly<Ref<UsageTimeRangeParams>>
  callerFilters: Readonly<Ref<CallerUsageFilters>>
  refreshTimeRangeEnd: () => void
}

type LoadingAnalytics<T> = Omit<T, 'analytics'> & { analytics: AnalyticsMetadata | null }

export function useUsageAnalytics(options: UseUsageAnalyticsOptions) {
  const analyticsLoading = shallowRef(true)
  const summary = shallowRef(emptySummary())
  const insights = shallowRef(emptyInsights())
  const providerQuery = shallowRef('')
  const diagnosticDimension = shallowRef('model')
  const refreshing = shallowRef(false)
  let analyticsRequestId = 0
  let diagnosticRequestId = 0
  let analyticsController: AbortController | undefined
  let diagnosticController: AbortController | undefined
  const scopedParams = () => ({
    ...options.timeRangeParams.value,
    ...options.callerFilters.value,
    ...(providerQuery.value
      ? { provider: providerQuery.value }
      : {}),
  })

  function loadUsageAnalytics() {
    options.refreshTimeRangeEnd()
    return loadAnalytics(scopedParams())
  }

  async function refreshUsageAnalytics() {
    if (refreshing.value || analyticsLoading.value)
      return
    refreshing.value = true
    try {
      await withMinimumDuration(loadUsageAnalytics)
    }
    finally {
      refreshing.value = false
    }
  }

  async function loadAnalytics(globalParams: ReturnType<typeof scopedParams>) {
    const requestId = ++analyticsRequestId
    const diagnosticsId = ++diagnosticRequestId
    analyticsController?.abort()
    diagnosticController?.abort()
    analyticsController = new AbortController()
    const requestOptions = { signal: analyticsController.signal }
    const dimension = diagnosticDimension.value
    analyticsLoading.value = true
    summary.value = emptySummary()
    insights.value = emptyInsights()
    try {
      const [nextSummary, overview, diagnostics] = await Promise.all([
        getUsageRecordSummary(globalParams, requestOptions),
        getUsageRecordInsightsOverview(globalParams, requestOptions),
        getUsageRecordInsightsDiagnostics({
          ...globalParams,
          dimension,
        }, requestOptions),
      ])
      if (requestId !== analyticsRequestId)
        return

      summary.value = nextSummary
      insights.value = {
        overview,
        diagnostics:
          diagnosticsId === diagnosticRequestId && dimension === diagnosticDimension.value
            ? diagnostics
            : insights.value.diagnostics,
      }
    }
    catch {
      if (requestId === analyticsRequestId) {
        summary.value = emptySummary()
        insights.value = emptyInsights()
      }
    }
    finally {
      if (requestId === analyticsRequestId) {
        analyticsLoading.value = false
      }
    }
  }

  async function loadDiagnostics() {
    const requestId = ++diagnosticRequestId
    diagnosticController?.abort()
    diagnosticController = new AbortController()
    const dimension = diagnosticDimension.value
    const params = scopedParams()
    try {
      const diagnostics = await getUsageRecordInsightsDiagnostics({
        ...params,
        dimension,
      }, { signal: diagnosticController.signal })
      if (requestId !== diagnosticRequestId || dimension !== diagnosticDimension.value)
        return
      insights.value = {
        ...insights.value,
        diagnostics,
      }
    }
    catch {}
  }

  onMounted(() => {
    void loadUsageAnalytics()
  })
  watch([options.callerFilters, providerQuery], () => {
    void loadUsageAnalytics()
  })
  watch(diagnosticDimension, () => {
    void loadDiagnostics()
  })

  onScopeDispose(() => {
    analyticsRequestId += 1
    diagnosticRequestId += 1
    analyticsController?.abort()
    diagnosticController?.abort()
  })

  return {
    analyticsLoading,
    summary,
    insights,
    providerQuery,
    diagnosticDimension,
    refreshing,
    loadUsageAnalytics,
    refreshUsageAnalytics,
  }
}

function emptySummary() {
  const summary: LoadingAnalytics<Awaited<ReturnType<typeof getUsageRecordSummary>>> = {
    analytics: null,
    totalRequests: '0',
    inputTokens: '0',
    outputTokens: '0',
    cachedInputTokens: '0',
    cacheWriteInputTokens: '0',
    totalTokens: '0',
    averageLatencyMs: '0 ms',
  }
  return summary
}

function emptyInsights() {
  return {
    overview: emptyOverview(),
    diagnostics: emptyDiagnostics(),
  }
}

function emptyOverview() {
  const overview: LoadingAnalytics<Awaited<ReturnType<typeof getUsageRecordInsightsOverview>>> = {
    analytics: null,
    granularity: '1d',
    health: {
      totalRequests: 0,
      successRequests: 0,
      failedRequests: 0,
      cancelledRequests: 0,
      incompleteRequests: 0,
      callerErrorRequests: 0,
      successRate: 0,
      completionRate: 0,
      requestChangeRate: null,
      successRateChange: null,
      points: [],
    },
    performance: {
      latencyP50Ms: null,
      latencyP95Ms: null,
      latencyP99Ms: null,
      firstTokenP50Ms: null,
      firstTokenP95Ms: null,
      firstTokenP99Ms: null,
      admissionDecisionP50Ms: null,
      admissionDecisionP95Ms: null,
      accountSelectionWaitP50Ms: null,
      accountSelectionWaitP95Ms: null,
      outputThroughputP10: null,
      outputThroughputP50: null,
      outputThroughputP90: null,
      capacityUtilization: null,
      capacityUtilizationP95: null,
      latencyCoverage: 0,
      firstTokenCoverage: 0,
      admissionDecisionCoverage: 0,
      accountSelectionWaitCoverage: 0,
      capacityCoverage: 0,
      load: {
        averageConcurrency: 0,
        peakConcurrency: 0,
        admissionDecisionP95Ms: null,
        accountSelectionWaitP95Ms: null,
      },
      points: [],
    },
    cost: {
      estimatedCost: null,
      standardCost: null,
      noCacheCost: null,
      cacheSavings: null,
      tierPremium: null,
      costPerRequest: null,
      costPerSuccessfulRequest: null,
      tokensPerRequest: 0,
      cachedTokenRate: 0,
      cacheHitRequestRate: 0,
      inputTokens: 0,
      outputTokens: 0,
      cachedInputTokens: 0,
      totalTokens: 0,
      points: [],
      coverage: {
        known: 0,
        partial: 0,
        unknown: 0,
        notBillable: 0,
      },
    },
  }
  return overview
}

function emptyDiagnostics() {
  const diagnostics: LoadingAnalytics<Awaited<ReturnType<typeof getUsageRecordInsightsDiagnostics>>> = {
    analytics: null,
    dimension: 'model',
    items: [],
  }
  return diagnostics
}
