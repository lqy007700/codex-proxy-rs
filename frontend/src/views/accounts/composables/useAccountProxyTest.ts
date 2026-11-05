import type { OutboundProxyRecord, OutboundProxyTest } from '@/api'
import { computed, shallowRef, watch } from 'vue'
import { probeProxy, testProxy } from '@/api'
import { useRequestState } from '@/composables/useRequestState'
import { notification as ZNotification } from '@/utils/notification'
import { errorMessage } from '@/utils/operation'

export function useAccountProxyTest(options: {
  target: () => OutboundProxyRecord | 'direct' | null
  accountId: () => string | undefined
  onTested: (proxy: OutboundProxyRecord) => void
  onConflict: () => Promise<void>
}) {
  const request = useRequestState(cause => ZNotification.error({
    message: errorMessage(cause),
  }))
  const result = shallowRef<OutboundProxyTest | null>(null)
  const directResult = shallowRef<OutboundProxyTest | null>(null)
  const targetId = computed(() => {
    const target = options.target()
    return typeof target === 'object'
      ? target?.id
      : target
  })

  watch([targetId, options.accountId], ([, accountId], [, previousAccountId]) => {
    request.invalidate()
    result.value = null
    if (accountId !== previousAccountId)
      directResult.value = null
    request.error.value = ''
  }, { flush: 'sync' })

  async function test() {
    const target = options.target()
    if (!target || request.loading.value)
      return
    const requestId = request.start()
    result.value = null
    const requestOptions = { signal: request.signal, silent: true }
    try {
      if (target === 'direct') {
        const response = await probeProxy({ proxyUrl: '', detectLocation: false }, requestOptions)
        if (request.isCurrent(requestId)) {
          result.value = response
          directResult.value = response
        }
      }
      else {
        const [response] = await testProxy([
          { id: target.id, revision: target.revision, detectLocation: false },
        ], requestOptions)
        if (!request.isCurrent(requestId))
          return
        if (response?.status !== 'succeeded') {
          if (response?.status === 'failed' && response.httpStatus === 409)
            await options.onConflict()
          throw new Error(response?.status === 'failed'
            ? response.message
            : '未取得测试结果，请重新测试')
        }
        options.onTested(response.data)
        result.value = response.data.lastTest
      }
      if (result.value && !result.value.success)
        request.fail(requestId, new Error(result.value.message || '代理测试失败'))
    }
    catch (error) {
      request.fail(requestId, error)
    }
    finally {
      request.finish(requestId)
    }
  }

  return {
    result,
    directResult,
    testing: request.loading,
    test,
  }
}
