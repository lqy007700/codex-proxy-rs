import type { Ref } from 'vue'
import type { Account, UsageListRecord } from '@/api'
import type { AffinityOperation, AffinitySession } from '@/api/modules/affinity'
import { computed, shallowRef, watch } from 'vue'
import { getAccounts } from '@/api'
import { applyAffinity, getAffinitySessions, previewAffinity } from '@/api/modules/affinity'
import { usageAccountText } from '@/components/usage/shared/presenter'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { useRequestState } from '@/composables/useRequestState'
import { accountIdentityLabel } from '@/utils/accountIdentity'
import { affinityTargetDisabled } from '@/utils/affinity'
import { notification } from '@/utils/notification'
import { errorMessage, generateRequestId } from '@/utils/operation'

export function useUsageAffinity(record: Readonly<Ref<UsageListRecord | null>>, open: Ref<boolean>) {
  const binding = shallowRef<AffinitySession | null>(null)
  const accounts = shallowRef<Account[]>([])
  const targetAccount = shallowRef<string>()
  const operation = shallowRef<AffinityOperation | null>(null)
  const saveError = shallowRef('')
  const bindingQuery = useRequestState()
  const accountQuery = useRequestState()
  const mutation = useAsyncAction({
    onError: cause => saveError.value = errorMessage(cause),
  })
  const result = computed(() => binding.value && operation.value
    ? operation.value.results[`${binding.value.provider}:${binding.value.reference}`]
    : undefined)
  const rejected = computed(() => operation.value?.plan.items.some(item => item.rejection)
    || (result.value?.auditRecorded && ['conflict', 'rejected'].includes(result.value.status)))
  const accountOptions = computed(() => accounts.value.map(account => ({
    label: accountIdentityLabel(account),
    value: account.id,
    disabled: affinityTargetDisabled(account),
  })))
  const canSave = computed(() => binding.value && targetAccount.value && !rejected.value
    && !bindingQuery.loading.value && !accountQuery.loading.value && !mutation.loading.value)

  function accountLabel(id: string) {
    const account = accounts.value.find(account => account.id === id)
    if (account)
      return accountIdentityLabel(account)
    return record.value?.accountId === id
      ? usageAccountText(record.value)
      : id
  }

  async function loadAccounts(search = '') {
    const provider = binding.value?.provider
    if (!open.value || !provider)
      return
    const requestId = accountQuery.start()
    try {
      const page = await getAccounts({
        page: 1,
        pageSize: 50,
        provider,
        search: search.trim() || undefined,
      }, { signal: accountQuery.signal, silent: true })
      if (accountQuery.isCurrent(requestId))
        accounts.value = page.items.filter(account => account.schedulingRole !== 'fallback')
    }
    catch (cause) { accountQuery.fail(requestId, cause) }
    finally { accountQuery.finish(requestId) }
  }

  async function loadBinding() {
    if (!open.value || mutation.loading.value)
      return
    binding.value = null
    accounts.value = []
    targetAccount.value = undefined
    operation.value = null
    saveError.value = ''
    accountQuery.invalidate()
    const requestId = bindingQuery.start()
    const caller = record.value?.callerIdentity
    const provider = record.value?.provider
    try {
      if (caller?.mode !== 'newApi' || !provider)
        return
      let cursor: string | undefined
      do {
        // 以稳定调用方身份定位主绑定，历史请求及临时分流账号不能缩小该范围
        const page = await getAffinitySessions({
          bindingScope: 'token',
          mode: 'newApi',
          sourceId: caller.sourceId,
          userId: caller.userId,
          tokenId: caller.tokenId,
          provider,
          cursor,
        }, { signal: bindingQuery.signal, silent: true })
        if (!bindingQuery.isCurrent(requestId))
          return
        if (page.items[0]) {
          binding.value = page.items[0]
          void loadAccounts()
          return
        }
        cursor = page.nextCursor ?? undefined
      } while (cursor)
    }
    catch (cause) { bindingQuery.fail(requestId, cause) }
    finally { bindingQuery.finish(requestId) }
  }

  async function save() {
    const session = binding.value
    const accountId = targetAccount.value
    if (!canSave.value || !session || !accountId)
      return
    saveError.value = ''
    await mutation.run(async () => {
      if (!operation.value) {
        operation.value = await previewAffinity({
          operationId: generateRequestId(),
          directive: { action: 'target', accountId },
          selection: [
            { provider: session.provider, reference: session.reference, revision: session.revision },
          ],
        })
      }
      const rejection = operation.value.plan.items.find(item => item.rejection)?.rejection
      if (rejection) {
        saveError.value = rejection
        return
      }
      // 结果未知或审计待补写时复用原操作，避免重新预览覆盖已经生效的指令
      operation.value = await applyAffinity({ operationId: operation.value.plan.operationId })
      const applied = result.value
      if (applied?.status === 'applied' && applied.auditRecorded) {
        notification.success({ message: '绑定调整已保存，后续符合条件的新请求生效' })
        open.value = false
        return
      }
      saveError.value = applied?.status === 'applied'
        ? '绑定调整已保存，操作记录待补写，请重试保存'
        : `${applied?.message || '未收到操作结果，请重试保存'}${applied && !applied.auditRecorded
          ? '（操作记录待补写，请重试保存）'
          : ''}`
    })
  }

  watch([
    open,
    () => record.value?.id,
  ], ([visible]) => {
    if (visible) {
      void loadBinding()
    }
    else {
      bindingQuery.invalidate({ resetLoading: false })
      accountQuery.invalidate({ resetLoading: false })
    }
  }, { immediate: true, flush: 'sync' })

  return {
    binding,
    targetAccount,
    operation,
    saveError,
    rejected,
    accountOptions,
    canSave,
    accountLabel,
    loading: bindingQuery.loading,
    error: bindingQuery.error,
    accountsLoading: accountQuery.loading,
    accountsError: accountQuery.error,
    saving: mutation.loading,
    loadBinding,
    loadAccounts,
    save,
  }
}
