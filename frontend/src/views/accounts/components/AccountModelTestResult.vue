<script setup lang="ts">
import type { AccountModelTestDetail } from '@/api'
import { ZButton, ZTag } from '@codex-proxy/ui'
import { computed } from 'vue'

import { accountModelTestImageUrl } from '@/api'
import { useCopyText } from '@/composables/useCopyText'
import { formatDuration } from '@/utils/format'

const { result } = defineProps<{ result: AccountModelTestDetail }>()
const copyText = useCopyText()
const imageUrl = computed(() => result.hasImage
  ? accountModelTestImageUrl(result.id)
  : '')
const animationUrl = computed(() => {
  if (result.kind !== 'animation' || !result.text)
    return ''
  const source = result.text.trim()
  const lineEnd = source.indexOf('\n')
  const fence = '```'
  const svg = lineEnd > 0 && source.slice(0, lineEnd).trimEnd().toLowerCase() === `${fence}svg` && source.endsWith(fence)
    ? source.slice(lineEnd + 1, -fence.length).trim()
    : source
  if (!/^<svg(?:\s|>)/i.test(svg) || !svg.endsWith('</svg>'))
    return ''
  // SVG 仅通过图片上下文预览，浏览器禁用其中的脚本和外部资源
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`
})
</script>

<template>
  <section class="grid min-w-0 gap-3 rounded-z-card bg-z-fill-quaternary p-3">
    <div class="flex flex-wrap items-center gap-2">
      <ZTag v-if="result.status === 'running'" size="small" type="info">
        测试中
      </ZTag>
      <ZTag v-else-if="result.status === 'failed'" size="small" type="danger">
        测试失败
      </ZTag>
      <ZTag v-else size="small" type="success">
        测试完成
      </ZTag>
      <span class="wrap-anywhere font-mono text-z-sm text-z-text-secondary">{{ result.modelId }}</span>
      <span v-if="result.durationMs !== null" class="text-z-sm text-z-text-secondary">{{ formatDuration(result.durationMs) }}</span>
    </div>
    <p v-if="result.status === 'running'" class="m-0 text-z-sm text-z-text-secondary">
      关闭弹窗后测试会继续运行
    </p>
    <p v-if="result.error" class="m-0 wrap-anywhere text-z-sm text-z-error-text" role="alert">
      {{ result.error }}
    </p>
    <div v-if="imageUrl || animationUrl" class="grid justify-items-start gap-2">
      <img :src="imageUrl || animationUrl" alt="模型生成结果" class="max-h-80 w-auto max-w-full rounded-z object-contain">
      <a v-if="imageUrl" :href="imageUrl" download class="text-z-sm text-z-link">下载图片</a>
    </div>
    <div v-if="result.text !== null" class="grid min-w-0 gap-2">
      <div class="flex items-center justify-between gap-2">
        <span class="text-z-sm font-heavy text-z-text-secondary">原始回答</span>
        <ZButton size="small" @click="copyText(result.text!, { successText: '已复制回答' })">
          复制
        </ZButton>
      </div>
      <pre class="m-0 max-h-72 overflow-auto whitespace-pre-wrap wrap-anywhere rounded-z bg-z-bg-container p-3 font-mono text-z-sm" v-text="result.text" />
    </div>
  </section>
</template>
