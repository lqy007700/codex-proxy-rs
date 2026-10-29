<script setup lang="ts">
import type { EChartsOption } from 'echarts'

import type { getUsageRecordInsightsOverview } from '@/api'
import { ZCard, ZEmpty, ZSegmented } from '@codex-proxy/ui'

import { computed, shallowRef } from 'vue'
import BaseChart from '@/components/charts/BaseChart.vue'
import { requestActivityByBucket, zeroInactiveValues } from '@/components/charts/timeSeriesGap'
import { tooltipIndex, tooltipRows } from '@/components/charts/tooltip'
import { useChartPalette } from '@/composables/useChartPalette'
import { formatLocalizedCompactNumber as formatCompactNumber, formatDuration, formatPercent } from '@/utils/format'

import {
  formatDurationAxis,
  usageCategoryAxis,
  usageGapAwareLineSeries,
  usageLegend,
  usageTooltip,
  usageTooltipContent,
  usageTooltipItem,
  usageValueAxis,
} from '../utils/chart'

const props = withDefaults(
  defineProps<{
    performance: Performance
    activity: Activity
    loading?: boolean
  }>(),
  {
    loading: false,
  },
)

type Performance = Awaited<ReturnType<typeof getUsageRecordInsightsOverview>>['performance']
type PerformancePoint = Performance['points'][number]
type PerformanceMetric = Exclude<keyof PerformancePoint, 'bucket' | 'label' | 'load'>

interface PerformanceSeries {
  field: PerformanceMetric | keyof PerformancePoint['load']
  value: (point: Omit<PerformancePoint, 'bucket' | 'label'>) => number | null
  label: string
  color: string
  yAxisIndex: number
  format: (value: number | null) => string
}

type Activity = Awaited<ReturnType<typeof getUsageRecordInsightsOverview>>['health']['points']

const activeView = shallowRef('total')
const { palette } = useChartPalette()
const performancePoints = computed<PerformancePoint[]>(() => props.performance.points)
const requestActivity = computed(() => requestActivityByBucket(
  performancePoints.value.map(point => point.bucket),
  props.activity,
))

const viewOptions = computed(() => ([
  {
    label: '总耗时',
    value: 'total',
  },
  {
    label: '首字',
    value: 'firstToken',
  },
  {
    label: '速率',
    value: 'throughput',
  },
  { label: '调度', value: 'scheduling' },
  {
    label: '负载',
    value: 'load',
  },
]))

const seriesDefinitions = computed<PerformanceSeries[]>(() => {
  const theme = palette.value
  if (activeView.value === 'load') {
    return [
      {
        field: 'averageConcurrency',
        value: point => point.load.averageConcurrency,
        label: '平均并发',
        color: theme.info,
        yAxisIndex: 0,
        format: formatConcurrency,
      },
      {
        field: 'peakConcurrency',
        value: point => point.load.peakConcurrency,
        label: '峰值并发',
        color: theme.warning,
        yAxisIndex: 0,
        format: formatConcurrency,
      },
      {
        field: 'admissionDecisionP95Ms',
        value: point => point.load.admissionDecisionP95Ms,
        label: '准入 P95',
        color: theme.success,
        yAxisIndex: 1,
        format: formatDuration,
      },
      {
        field: 'accountSelectionWaitP95Ms',
        value: point => point.load.accountSelectionWaitP95Ms,
        label: '选号等待 P95',
        color: theme.reasoning,
        yAxisIndex: 1,
        format: formatDuration,
      },
    ]
  }
  if (activeView.value === 'scheduling') {
    return [
      {
        field: 'admissionDecisionP95Ms',
        value: point => point.admissionDecisionP95Ms,
        label: '准入 P95',
        color: theme.info,
        yAxisIndex: 0,
        format: formatDuration,
      },
      {
        field: 'accountSelectionWaitP95Ms',
        value: point => point.accountSelectionWaitP95Ms,
        label: '选号 P95',
        color: theme.warning,
        yAxisIndex: 0,
        format: formatDuration,
      },
      {
        field: 'capacityUtilizationP95',
        value: point => point.capacityUtilizationP95,
        label: '容量 P95',
        color: theme.success,
        yAxisIndex: 1,
        format: formatPercent,
      },
    ]
  }
  const throughput = activeView.value === 'throughput'
  const fields: PerformanceMetric[] = throughput
    ? ['outputThroughputP10', 'outputThroughputP50', 'outputThroughputP90']
    : activeView.value === 'firstToken'
      ? ['firstTokenP50Ms', 'firstTokenP95Ms', 'firstTokenP99Ms']
      : ['latencyP50Ms', 'latencyP95Ms', 'latencyP99Ms']
  const labels = throughput
    ? ['P10', 'P50', 'P90']
    : ['P50', 'P95', 'P99']
  const colors = [theme.info, theme.warning, theme.danger]
  return fields.map((field, index) => ({
    field,
    value: point => point[field],
    label: labels[index]!,
    color: colors[index]!,
    yAxisIndex: 0,
    format: throughput ? formatThroughput : formatDuration,
  }))
})

const selectedSeries = computed(() => seriesDefinitions.value.map((series) => {
  const values = performancePoints.value.map(series.value)
  return {
    ...series,
    // 并发包含之前时间桶开始的请求，不按本桶新请求数补零
    values: series.field === 'averageConcurrency' || series.field === 'peakConcurrency'
      ? values
      : zeroInactiveValues(values, requestActivity.value),
  }
}))

const summaryMetrics = computed(() => activeView.value === 'scheduling'
  ? [
      {
        label: '判定覆盖',
        value: formatPercent(props.performance.admissionDecisionCoverage),
      },
      {
        label: '选号覆盖',
        value: formatPercent(props.performance.accountSelectionWaitCoverage),
      },
      {
        label: '容量覆盖',
        value: formatPercent(props.performance.capacityCoverage),
      },
    ]
  : seriesDefinitions.value.map(series => ({
      label: series.label,
      value: series.format(series.value(props.performance)),
    })))

const hasData = computed(() => !props.loading
  && seriesDefinitions.value.some(series => performancePoints.value.some(point => series.value(point) != null)))

const chartOption = computed<EChartsOption>(() => {
  const theme = palette.value
  const load = activeView.value === 'load'
  const series = selectedSeries.value.flatMap((item, index) => usageGapAwareLineSeries(
    item.label,
    item.values,
    item.color,
    {
      yAxisIndex: item.yAxisIndex,
      area: index === 0
        ? 'subtle'
        : false,
    },
  ))

  return {
    animationDuration: 240,
    grid: {
      left: 0,
      right: 0,
      top: load ? 60 : 40,
      bottom: 0,
      outerBoundsMode: 'same',
      outerBoundsContain: 'axisLabel',
    },
    legend: usageLegend(theme, selectedSeries.value.map(series => series.label)),
    tooltip: usageTooltip(theme, formatTooltip),
    xAxis: usageCategoryAxis(performancePoints.value.map(point => point.label), theme),
    yAxis: load
      ? [
          {
            ...usageValueAxis(theme, formatCompactNumber, { min: 0 }),
            name: '并发',
            nameTextStyle: { color: theme.textMuted, fontSize: 10, align: 'left' },
          },
          {
            ...usageValueAxis(theme, formatDurationAxis, { min: 0, splitLine: false }),
            name: '耗时',
            nameTextStyle: { color: theme.textMuted, fontSize: 10, align: 'right' },
          },
        ]
      : activeView.value === 'scheduling'
        ? [
            usageValueAxis(theme, formatDurationAxis),
            usageValueAxis(theme, formatPercent, { min: 0, max: 1, splitLine: false }),
          ]
        : usageValueAxis(
            theme,
            activeView.value === 'throughput'
              ? value => `${formatCompactNumber(value)}/s`
              : formatDurationAxis,
          ),
    series,
  }
})

function formatTooltip(params: unknown) {
  const rows = tooltipRows(params)
  const index = tooltipIndex(rows[0])
  const point = performancePoints.value[index]
  if (!point)
    return ''

  return usageTooltipContent(palette.value, point.label, selectedSeries.value.map(series => usageTooltipItem(
    series.label,
    series.format(series.values[index] ?? null),
    series.color,
  )))
}

function formatConcurrency(value: number | null) {
  return value == null
    ? '—'
    : formatCompactNumber(value)
}

function formatThroughput(value: number | null) {
  return value == null
    ? '—'
    : `${formatCompactNumber(value)} tok/s`
}
</script>

<template>
  <ZCard
    as="article"
    title="响应性能"
    description="总耗时、首个 Token 耗时、速率与负载"
    class="min-h-90 xl:h-full"
  >
    <template #actions>
      <ZSegmented v-model="activeView" aria-label="性能指标" :options="viewOptions" :disabled="loading" class="w-84 max-w-full" />
    </template>

    <template #body>
      <div class="grid min-h-66 gap-3">
        <div
          v-if="hasData"
          class="grid gap-2 rounded-xl bg-cp-fill-quaternary/45 p-2"
          :class="activeView === 'load' ? 'grid-cols-2 sm:grid-cols-4' : 'grid-cols-3'"
        >
          <div v-for="metric in summaryMetrics" :key="metric.label" class="grid min-w-0 gap-1 px-2">
            <span class="truncate text-[10px] font-bold text-cp-text-quaternary" :title="metric.label">{{ metric.label }}</span>
            <strong class="truncate font-mono text-cp-base font-heavy tabular-nums text-cp-text" :title="metric.value">
              {{ metric.value }}
            </strong>
          </div>
        </div>
        <BaseChart v-if="hasData" :option="chartOption" :height="210" />
        <ZEmpty
          v-else
          size="small"
          surface="none"
          :title="
            loading
              ? '正在加载性能数据'
              : activeView === 'load'
                ? '当前范围暂无负载数据'
                : activeView === 'firstToken'
                  ? '当前范围暂无 TTFT 样本'
                  : activeView === 'throughput'
                    ? '当前范围暂无速率样本'
                    : activeView === 'scheduling'
                      ? '当前范围没有调度或容量样本'
                      : '当前范围暂无延迟样本'
          "
          class="h-52.5 place-content-center"
        />
      </div>
    </template>
  </ZCard>
</template>
