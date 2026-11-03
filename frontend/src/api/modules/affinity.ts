import type { RequestOptions } from '../request'
import request from '../request'

export type CallerIdentityPolicy = { mode: 'direct' } | { mode: 'newApi', sourceId: string, allowUnidentified: boolean }
export type CallerIdentity = { mode: 'direct', keyId: string } | { mode: 'newApi', sourceId: string, userId: string, tokenId: string } | { mode: 'unidentified', requestId: string }
export type BindingDirective = { action: 'clear' } | { action: 'reassign' } | { action: 'target', accountId: string }
export interface IdentitySource { id: string, name: string, enabled: boolean }
export interface AffinitySession {
  reference: string
  provider: string
  observation: { caller: CallerIdentity, clientKeyId: string, requestId: string, actualAccountId: string }
  accountId: string
  revision: string
  directive: BindingDirective | null
  firstObservedAtDisplay: string
  lastAdmissionAtDisplay: string
  expiresAtDisplay: string
  firstObservedAt: string
  lastAdmissionAt: string
  expiresAt: string
}
export interface AffinityFilters {
  mode?: string
  sourceId?: string
  clientKeyId?: string
  userId?: string
  tokenId?: string
  provider?: string
  accountId?: string
  reference?: string
  requestId?: string
  status?: string
}
export interface AffinityPage { items: AffinitySession[], nextCursor: string | null, coverage: 'observedOnly' }
export interface AffinityResult { provider: string, reference: string, status: 'applied' | 'conflict' | 'rejected' | 'failed', message: string, auditRecorded: boolean }
export interface AffinityOperation {
  plan: {
    operationId: string
    directive: BindingDirective
    items: { session: AffinitySession, nextRevision: string, rejection: string | null }[]
    createdAtDisplay: string
    expiresAtDisplay: string
    createdAt: string
    expiresAt: string
  }
  results: Record<string, AffinityResult>
}

export function getAffinitySessions(params: AffinityFilters & { cursor?: string }, options: RequestOptions = {}) {
  return request<AffinityPage>({
    url: '/api/admin/affinity/sessions',
    method: 'GET',
    params,
    ...options,
  })
}
export function getIdentitySources(options: RequestOptions = {}) {
  return request<IdentitySource[]>({
    url: '/api/admin/affinity/sources',
    method: 'GET',
    ...options,
  })
}
export function saveIdentitySource(data: IdentitySource) {
  return request<{ saved: boolean }>({
    url: '/api/admin/affinity/sources/save',
    method: 'POST',
    data,
  })
}
export function previewAffinity(data: { operationId: string, directive: BindingDirective, selection: { provider: string, reference: string, revision: string }[] }) {
  return request<AffinityOperation>({
    url: '/api/admin/affinity/preview',
    method: 'POST',
    data,
  })
}
export function applyAffinity(data: { operationId: string }) {
  return request<AffinityOperation>({
    url: '/api/admin/affinity/apply',
    method: 'POST',
    data,
    retry: false,
  })
}
export function getAffinityOperations(options: RequestOptions = {}) {
  return request<AffinityOperation[]>({
    url: '/api/admin/affinity/operations',
    method: 'GET',
    ...options,
  })
}
