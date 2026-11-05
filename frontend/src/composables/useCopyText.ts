import { useClipboard } from '@vueuse/core'

import { notification as ZNotification } from '@/utils/notification'

interface CopyTextOptions {
  successText?: string
  notifyWhenEmpty?: boolean
}

export function useCopyText() {
  // 普通 HTTP 页面没有 Clipboard API，启用 VueUse 内置的兼容复制路径。
  const { copy } = useClipboard({ legacy: true })

  return async function copyText(value: string, options: CopyTextOptions = {}) {
    if (!value) {
      if (options.notifyWhenEmpty) {
        ZNotification.error({
          message: '没有可复制的内容',
        })
      }
      return
    }
    try {
      await copy(value)
      ZNotification.success({ message: options.successText ?? '已复制' })
    }
    catch {
      ZNotification.error({
        message: '无法写入剪贴板',
      })
    }
  }
}
