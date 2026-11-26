import type { FormRule, FormRules } from '@codex-proxy/ui'
import type { ApiKeyFormValue } from './useApiKeyEditor'

export const apiKeyLimitFields = [
  'dailyLimitUsd',
  'weeklyLimitUsd',
  'maxConcurrency',
  'requestsPerMinute',
] as const

export type ApiKeyLimitField = (typeof apiKeyLimitFields)[number]

function amountRule(label: string): FormRule {
  return {
    trigger: ['blur', 'change'],
    transform: value => String(value ?? '').trim(),
    pattern: /^\d{1,10}(?:\.\d{1,10})?$/,
    message: `${label}需为最多 10 位整数和 10 位小数的非负金额`,
  }
}

function integerRule(label: string): FormRule {
  return {
    trigger: ['blur', 'change'],
    type: 'integer',
    transform: Number,
    min: 0,
    max: Number.MAX_SAFE_INTEGER,
    message: `${label}需为非负安全整数`,
  }
}

export function useApiKeyLimitRules(): FormRules<Record<ApiKeyLimitField, string>> {
  return {
    dailyLimitUsd: amountRule('每日限额'),
    weeklyLimitUsd: amountRule('每周限额'),
    maxConcurrency: integerRule('最大并发'),
    requestsPerMinute: integerRule('每分钟请求数'),
  }
}

export function useApiKeyFormRules(): FormRules<ApiKeyFormValue> {
  return {
    name: {
      required: true,
      whitespace: true,
      message: '请输入 API Key 名称',
      trigger: ['blur', 'change'],
    },
    customKey: {
      trigger: ['blur', 'change'],
      pattern: /^[\x21-\x7E]+$/,
      message: '自定义 Key 只能包含不带空格的可见 ASCII 字符',
    },
    ...useApiKeyLimitRules(),
  }
}
