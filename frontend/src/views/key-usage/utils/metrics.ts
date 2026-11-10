import type { KeyUsageMetrics } from '@/api/modules/key-usage'
import { ArrowDown, ArrowUp, Brain, Database, Sparkles } from '@lucide/vue'
import { usageFreshInputTokens } from '@/components/usage/shared/presenter'

// 卡片、曲线和图例共用语义与主题色，避免同一指标出现不同颜色。
export const keyUsageTokenMetrics = [
  {
    key: 'inputTokens',
    label: '新增输入',
    icon: ArrowDown,
    tone: 'text-z-success-text',
    colorToken: 'color-success-text',
    fallback: '#12B981',
  },
  {
    key: 'outputTokens',
    label: '输出',
    icon: ArrowUp,
    tone: 'text-z-warning-text',
    colorToken: 'color-warning-text',
    fallback: '#F59E0B',
  },
  {
    key: 'cacheWriteInputTokens',
    label: '缓存创建',
    icon: Database,
    tone: 'text-z-info-text',
    colorToken: 'color-info-text',
    fallback: '#5983F4',
  },
  {
    key: 'cachedInputTokens',
    label: '缓存命中',
    icon: Sparkles,
    tone: 'text-z-purple-text',
    colorToken: 'color-purple-text',
    fallback: '#722ED1',
  },
  {
    key: 'reasoningOutputTokens',
    label: '推理',
    icon: Brain,
    tone: 'text-z-cyan-text',
    colorToken: 'color-cyan-text',
    fallback: '#13C2C2',
  },
] as const

export function keyUsageTokenValue(metrics: KeyUsageMetrics, key: typeof keyUsageTokenMetrics[number]['key']) {
  return key === 'inputTokens'
    ? usageFreshInputTokens(metrics.inputTokens, metrics.cachedInputTokens, metrics.cacheWriteInputTokens)
    : metrics[key]
}
