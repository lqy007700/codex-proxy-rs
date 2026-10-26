import type { Account, ApiKeyConfiguration } from '@/api'
import { accountModelIdError } from './modelAccess'

export function isApiKeyAccount(account: Pick<Account, 'provider' | 'authenticationKind'> | null | undefined): boolean {
  return (account?.provider === 'openai' || account?.provider === 'glm') && account.authenticationKind === 'api_key'
}

export function isOpenAiOAuthAccount(account: Pick<Account, 'provider' | 'authenticationKind'> | null | undefined): boolean {
  return account?.provider === 'openai' && account.authenticationKind === 'oauth'
}

export interface ApiKeyAccountForm extends Omit<ApiKeyConfiguration, 'models'> {
  modelsText: string
  name: string
  apiKey: string
}

export function emptyApiKeyAccountForm(provider?: string): ApiKeyAccountForm {
  return { name: '', base_url: provider === 'glm' ? 'https://open.bigmodel.cn/api/v1' : '', apiKey: '', transport: 'http', modelsText: provider === 'glm' ? 'glm-5.3' : '' }
}

export function parseApiKeyConfiguration(value: Record<string, unknown> | undefined, provider?: string): ApiKeyConfiguration | undefined {
  if (!value || typeof value.base_url !== 'string' || (value.transport !== 'http' && value.transport !== 'prefer_websocket'))
    return undefined
  if (provider === 'glm') {
    if (!Array.isArray(value.models) || value.models.some(model => typeof model !== 'string') || glmModelListError(value.models))
      return undefined
    return { base_url: value.base_url, transport: value.transport, models: [...value.models] }
  }
  return { base_url: value.base_url, transport: value.transport }
}

export function apiKeyAccountError(form: ApiKeyAccountForm, editing = false, provider?: string): string | undefined {
  if (!editing && !form.name.trim())
    return '请输入账号名称'
  if (form.base_url.trim().length > 2048)
    return '上游 API 地址不能超过 2048 个字符'
  try {
    const url = new URL(form.base_url)
    if (!['https:', 'http:'].includes(url.protocol) || !url.hostname || url.username || url.password || url.search || url.hash)
      return '请输入不含认证、查询参数或片段的 HTTP 或 HTTPS 地址'
  }
  catch {
    return '请输入完整的上游 API 地址'
  }
  if (!editing && !form.apiKey)
    return '请输入 API Key'
  if (form.apiKey && (!/^[\x21-\x7E]+$/.test(form.apiKey) || form.apiKey.length > 16384))
    return 'API Key 不能包含空格或控制字符'
  if (provider === 'glm')
    return glmModelListError(apiKeyAccountModels(form))
  return undefined
}

export function apiKeyAccountModels(form: ApiKeyAccountForm): string[] {
  return form.modelsText.split(/\r?\n/).map(model => model.trim()).filter(Boolean)
}

function glmModelListError(models: string[]): string | undefined {
  if (!models.length || models.length > 128)
    return '请填写 1–128 个 GLM 模型 ID'
  if (models.some(model => !model.startsWith('glm-') || accountModelIdError(model)))
    return '请填写以 glm- 开头的有效模型 ID'
  if (new Set(models).size !== models.length)
    return '模型 ID 不能重复'
}
