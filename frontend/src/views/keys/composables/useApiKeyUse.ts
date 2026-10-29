import type { ApiKey } from '@/api'
import { shallowRef } from 'vue'
import { revealApiKey } from '@/api'
import { useCopyText } from '@/composables/useCopyText'
import { useRequestState } from '@/composables/useRequestState'
import { buildCodexCcSwitchImportDeeplink, resolveServiceRootUrl } from '@/utils/client'
import { notification as ZNotification } from '@/utils/notification'

type KeyRevealAction = 'copy' | 'use' | 'import'

// 明文读取、复制、展示与导入共用生命周期，列表数据不保存明文
export function useApiKeyUse() {
  const copyText = useCopyText()
  const revealingKeys = shallowRef(new Map<string, KeyRevealAction>())
  const revealRequest = useRequestState()
  const showUseKeyModal = shallowRef(false)
  const selectedUseKey = shallowRef<(ApiKey & { key: string }) | null>(null)
  const showCreatedKeyModal = shallowRef(false)
  const createdKey = shallowRef('')
  const createdKeyName = shallowRef('')

  function showCreatedKey(key: string, name: string) {
    createdKey.value = key
    createdKeyName.value = name
    showCreatedKeyModal.value = true
  }

  const openAiBaseUrl = `${resolveServiceRootUrl()}/v1`

  function importCreatedKeyToCcs() {
    if (!createdKey.value)
      return

    window.location.href = buildCodexCcSwitchImportDeeplink({
      apiKey: createdKey.value,
      baseUrl: openAiBaseUrl,
      providerName: createdKeyName.value || 'codex-proxy-rs',
    })
  }

  function openUseKeyModal(apiKey: ApiKey) {
    return withPlaintextKey(apiKey, 'use', (key) => {
      selectedUseKey.value = { ...apiKey, key }
      showUseKeyModal.value = true
    })
  }

  function importToCcs(apiKey: ApiKey) {
    return withPlaintextKey(apiKey, 'import', (key) => {
      window.location.href = buildCodexCcSwitchImportDeeplink({
        apiKey: key,
        baseUrl: openAiBaseUrl,
        providerName: apiKey.name || apiKey.prefix || 'codex-proxy-rs',
      })
    })
  }

  async function copyToClipboard(text: string) {
    await copyText(text, { successText: '已复制到剪贴板', emptyErrorText: '复制失败' })
  }

  async function withPlaintextKey(apiKey: ApiKey, action: KeyRevealAction, use: (key: string) => void | Promise<void>) {
    if (revealingKeys.value.has(apiKey.id))
      return
    // 页面离开或新的操作意图使旧结果失效，避免迟到结果弹出旧密钥
    const requestId = revealRequest.start()
    revealingKeys.value = new Map(revealingKeys.value).set(apiKey.id, action)
    try {
      const result = await revealApiKey({ id: apiKey.id }, { signal: revealRequest.signal })
      if (!revealRequest.isCurrent(requestId))
        return
      if (!result.plaintextKey) {
        ZNotification.error({ message: '完整 API Key 不可用' })
        return
      }
      await use(result.plaintextKey)
    }
    catch (error) {
      revealRequest.fail(requestId, error)
    }
    finally {
      const pending = new Map(revealingKeys.value)
      pending.delete(apiKey.id)
      revealingKeys.value = pending
      revealRequest.finish(requestId)
    }
  }

  function copyApiKey(apiKey: ApiKey) {
    return withPlaintextKey(apiKey, 'copy', copyToClipboard)
  }

  function clearCreatedKey() {
    createdKey.value = ''
    createdKeyName.value = ''
  }

  return {
    showUseKeyModal,
    selectedUseKey,
    showCreatedKeyModal,
    createdKey,
    revealingKeys,
    openAiBaseUrl,
    showCreatedKey,
    clearCreatedKey,
    copyToClipboard,
    copyApiKey,
    importCreatedKeyToCcs,
    openUseKeyModal,
    importToCcs,
  }
}
