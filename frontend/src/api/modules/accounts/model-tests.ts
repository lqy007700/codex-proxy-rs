import type { RequestOptions } from '../../request'
import type { AccountPageMeta } from './management'
import { API_BASE_URL } from '../../constants'
import request from '../../request'

export type AccountModelTestKind = 'text' | 'image' | 'animation'
export type AccountModelTestStatus = 'running' | 'succeeded' | 'failed'

export interface AccountModelTestInput {
  accountId: string
  modelId: string
  kind: AccountModelTestKind
  prompt: string
}

export interface AccountModelTestSummary {
  id: string
  accountId: string
  accountName: string
  modelId: string
  kind: AccountModelTestKind
  status: AccountModelTestStatus
  startedAtDisplay: string
  durationMs: number | null
}

export interface AccountModelTestDetail extends AccountModelTestSummary {
  prompt: string
  error: string | null
  text: string | null
  hasImage: boolean
}

const path = '/api/admin/accounts/model-tests'

export function startAccountModelTest(data: AccountModelTestInput) {
  return request<{ testId: string }>({
    url: path,
    method: 'POST',
    data,
  })
}

export function getAccountModelTests(params: { accountId?: string, page: number }, options: RequestOptions = {}) {
  return request<{ items: AccountModelTestSummary[], page: AccountPageMeta }>({
    url: path,
    method: 'GET',
    params,
    ...options,
  })
}

export function getAccountModelTestDetail(testId: string, options: RequestOptions = {}) {
  return request<AccountModelTestDetail>({
    url: `${path}/detail`,
    method: 'GET',
    params: { testId },
    ...options,
  })
}

export function accountModelTestImageUrl(testId: string) {
  return `${API_BASE_URL}${path}/image?${new URLSearchParams({ testId })}`
}
