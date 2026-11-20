<script setup lang="ts">
import type { AccountModelTestPreview } from '../utils/modelTestPreview'

defineProps<{ preview: AccountModelTestPreview, thumbnail?: boolean }>()
</script>

<template>
  <div
    class="overflow-hidden rounded-z bg-white"
    :class="thumbnail ? 'h-[72px] w-32' : 'w-full'"
    :inert="thumbnail || undefined"
  >
    <!-- data 文档自带独立来源，保留 SVG 声明式动画，沙箱继续禁用脚本 -->
    <iframe
      v-if="preview.kind === 'document'"
      title="动画预览"
      sandbox="allow-same-origin"
      referrerpolicy="no-referrer"
      :tabindex="thumbnail ? -1 : undefined"
      :src="`data:text/html;charset=utf-8,${encodeURIComponent(preview.source)}`"
      class="block border-0"
      :class="thumbnail
        ? 'pointer-events-none h-[288px] w-[512px] origin-top-left scale-25'
        : 'h-[min(62vh,42rem)] w-full'"
    />
    <img
      v-else
      :src="preview.source"
      alt="模型生成结果"
      class="block object-contain"
      :class="thumbnail ? 'h-full w-full' : 'mx-auto max-h-[62vh] max-w-full'"
    >
  </div>
</template>
