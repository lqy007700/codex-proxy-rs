<script setup lang="ts">
import type { ThemeName } from '@codex-proxy/ui/theme'
import type { CSSProperties } from 'vue'
import type { ThemeEditorPreview } from '../composables/useThemeEditor'

import { ZSegmented } from '@codex-proxy/ui'
import { defineAsyncComponent, onBeforeUnmount, onMounted, shallowRef } from 'vue'

import ThemePreviewCanvas from './ThemePreviewCanvas.vue'
import ThemePreviewError from './ThemePreviewError.vue'

defineProps<{
  theme: ThemeName
  style: CSSProperties
}>()
const previewReady = shallowRef(false)
const stylesRevision = shallowRef(0)
let paintFrame = 0

// 先绘制画布和控件，再挂载预览；缓存命中时也不阻塞外框首帧
onMounted(() => {
  paintFrame = requestAnimationFrame(() => {
    paintFrame = requestAnimationFrame(() => {
      previewReady.value = true
    })
  })
})
onBeforeUnmount(() => cancelAnimationFrame(paintFrame))

const ThemeDashboardPreview = defineAsyncComponent({
  loader: async () => {
    const component = await import('./ThemeDashboardPreview.vue')
    stylesRevision.value++
    return component
  },
  errorComponent: ThemePreviewError,
  timeout: 30000,
})
const ThemeComponentPreview = defineAsyncComponent({
  loader: async () => {
    const component = await import('./ThemeComponentPreview.vue')
    stylesRevision.value++
    return component
  },
  errorComponent: ThemePreviewError,
  timeout: 30000,
})

const preview = defineModel<ThemeEditorPreview>({ required: true })

const previewOptions = [
  { label: '首页画板', value: 'page' },
  { label: '组件概览', value: 'components' },
]
</script>

<template>
  <section class="grid h-full min-h-0 grid-rows-[auto_minmax(0,1fr)] gap-3 overflow-hidden" aria-label="主题实时预览">
    <header class="shrink-0 flex flex-wrap items-center justify-between gap-3 rounded-cp-lg bg-cp-bg-container px-3 py-2.5 shadow-cp-tertiary">
      <div>
        <strong class="block text-cp-sm font-heavy text-cp-text">实时预览</strong>
        <span class="mt-0.5 block text-[9px] font-emphasis text-cp-text-quaternary">真实组件在影子环境隔离渲染，拖动画板并缩放检查细节</span>
      </div>
      <ZSegmented v-model="preview" aria-label="预览类型" size="small" :options="previewOptions" />
    </header>

    <ThemePreviewCanvas
      class="h-full min-h-0"
      :theme="theme"
      :style="style"
      :styles-revision="stylesRevision"
    >
      <template v-if="previewReady">
        <ThemeDashboardPreview v-if="preview === 'page'" />
        <ThemeComponentPreview v-else />
      </template>
    </ThemePreviewCanvas>
  </section>
</template>
