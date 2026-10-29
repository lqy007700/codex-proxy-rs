import type { AccountErrorProtection, ProtectionCondition, ProtectionField, ProtectionOperator, ProtectionRule } from '@/api/modules/settings/protection'

export type ProtectionDraft = Omit<AccountErrorProtection, 'threshold' | 'windowSeconds' | 'durationSeconds' | 'probeModel'> & {
  threshold: number | null
  windowSeconds: number | null
  durationSeconds: number | null
  probeModel: string
}

export const protectionLimits = {
  rules: 32,
  conditions: 8,
  values: 16,
  name: 128,
  textBytes: 64 * 1024,
}

export const protectionFieldLabels: Record<ProtectionField, string> = {
  provider: '平台',
  status: '状态码',
  code: '错误码',
  type: '错误类型',
  message: '错误信息',
}

export const protectionOperatorLabels: Record<ProtectionOperator, string> = {
  equals: '等于',
  contains: '包含',
}

function exceedsCharacters(value: string, max: number): boolean {
  let length = 0
  for (const _ of value) {
    if (++length > max)
      return true
  }
  return false
}

// 与服务端 char::is_whitespace 使用同一 Unicode 空白集合
export function trimProtectionText(value: string): string {
  return value.replace(/^\p{White_Space}+|\p{White_Space}+$/gu, '')
}

export function createProtectionCondition(): ProtectionCondition {
  return {
    field: 'code',
    operator: 'equals',
    values: [''],
    ignoreCase: false,
  }
}

export function protectionConditionError(condition: ProtectionCondition): string {
  if (!condition.values.length || condition.values.length > protectionLimits.values)
    return `每个条件需有 1～${protectionLimits.values} 个候选值`
  if (condition.operator === 'contains' && (condition.field !== 'message' || condition.values.some(value => value === null || !trimProtectionText(value))))
    return '包含匹配需填写非空白错误信息，不能选择字段缺失'
  const max = condition.field === 'message'
    ? 1024
    : 256
  if (condition.values.some(value => value !== null && exceedsCharacters(value, max)))
    return `候选值不能超过 ${max} 个字符`
  if (condition.field === 'status' && condition.values.some(value => value !== null && !/^[1-5]\d{2}$/.test(value)))
    return '状态码需为 100～599，或选择字段缺失'
  return ''
}

export function protectionRuleStructureError(rule: ProtectionRule): string {
  if (!trimProtectionText(rule.name) || rule.name !== trimProtectionText(rule.name) || /\p{Cc}/u.test(rule.name) || exceedsCharacters(rule.name, protectionLimits.name))
    return `名称需为 1～${protectionLimits.name} 个字符，不能包含首尾空白或控制字符`
  if (!rule.conditions.length || rule.conditions.length > protectionLimits.conditions)
    return `每条规则需有 1～${protectionLimits.conditions} 个条件`
  if (rule.conditions.every(condition => condition.field === 'provider'))
    return '请添加状态码、错误码、错误类型或错误信息条件'
  return ''
}

export function protectionRuleError(rule: ProtectionRule): string {
  const error = protectionRuleStructureError(rule)
  if (error)
    return error
  for (const condition of rule.conditions) {
    const conditionError = protectionConditionError(condition)
    if (conditionError)
      return conditionError
  }
  return ''
}

export function protectionRulesError(policy: Pick<AccountErrorProtection, 'enabled' | 'rules'>): string {
  if (policy.enabled && !policy.rules.length)
    return '启用前请添加至少一条有效规则'
  if (policy.rules.length > protectionLimits.rules)
    return `最多添加 ${protectionLimits.rules} 条规则`
  for (const [index, rule] of policy.rules.entries()) {
    const error = protectionRuleError(rule)
    if (error)
      return `${trimProtectionText(rule.name) || String(index + 1)}：${error}`
  }
  return ''
}

export function protectionSettingsError(policy: ProtectionDraft): string {
  const { threshold, windowSeconds, durationSeconds } = policy
  if (threshold === null || windowSeconds === null || durationSeconds === null)
    return '请完整填写错误保护参数'
  if (!Number.isInteger(threshold) || threshold < 2 || threshold > 1000
    || !Number.isInteger(windowSeconds) || windowSeconds < 60 || windowSeconds > 3600
    || !Number.isInteger(durationSeconds) || durationSeconds < 300 || durationSeconds > 604800) {
    return '失败阈值应为 2～1000，计数有效期为 60～3600 秒，冷却时长为 300～604800 秒'
  }
  if (exceedsCharacters(trimProtectionText(policy.probeModel), 128) || /\p{Cc}/u.test(policy.probeModel))
    return '探测模型最多 128 个字符，不能包含控制字符'
  const rulesError = protectionRulesError(policy)
  if (rulesError)
    return rulesError
  const encoder = new TextEncoder()
  let bytes = encoder.encode(trimProtectionText(policy.probeModel)).byteLength
  for (const rule of policy.rules) {
    bytes += encoder.encode(rule.id).byteLength + encoder.encode(rule.name).byteLength
    for (const condition of rule.conditions) {
      for (const value of condition.values) {
        if (value !== null)
          bytes += encoder.encode(value).byteLength
        if (bytes > protectionLimits.textBytes)
          return '规则文本合计超过 64 KiB UTF-8，请缩短候选值或删除规则'
      }
    }
  }
  return ''
}

export function protectionRuleSummary(rule: ProtectionRule): string {
  return rule.conditions.map((condition) => {
    const candidates = condition.values.slice(0, 2).map(value => value === null
      ? '字段缺失'
      : trimProtectionText(value) === ''
        ? '空文本'
        : `“${value.length > 60
          ? `${Array.from(value.slice(0, 120)).slice(0, 60).join('')}…`
          : value}”`).join(' 或 ')
    const values = condition.values.length > 2
      ? `${candidates} 或其他 ${condition.values.length - 2} 项`
      : candidates
    const summary = `${protectionFieldLabels[condition.field]}${protectionOperatorLabels[condition.operator]}${condition.values.length > 1
      ? `(${values})`
      : values}`
    return condition.ignoreCase
      ? `${summary}（忽略大小写）`
      : summary
  }).join(' 且 ')
}
