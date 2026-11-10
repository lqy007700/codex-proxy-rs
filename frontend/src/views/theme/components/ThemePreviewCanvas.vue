<script setup lang="ts">
import type { ThemeScheme } from '@codex-proxy/ui/theme'
import type { CSSProperties } from 'vue'
import { ZButton, ZIconButton } from '@codex-proxy/ui'

import { Maximize2, Minus, Move, Plus } from '@lucide/vue'
import { computed, useTemplateRef } from 'vue'

import { useThemePreviewCanvas } from '../composables/useThemePreviewCanvas'
import ThemePreviewScope from './ThemePreviewScope.vue'

defineProps<{
  theme: ThemeScheme
  style: CSSProperties
  stylesRevision: number
}>()

const viewport = useTemplateRef<HTMLElement>('viewport')
const stage = useTemplateRef<HTMLElement>('stage')
const boardWidth = 1600
const boardHeight = 1808
const { canZoomIn, canZoomOut, dragging, fitToViewport, resetScale, scale, scalePercent, zoomIn, zoomOut }
  = useThemePreviewCanvas({
    viewport,
    stage,
    boardWidth,
    boardHeight,
  })

const stageStyle = computed<CSSProperties>(() => ({
  width: `${boardWidth * scale.value}px`,
  height: `${boardHeight * scale.value}px`,
}))
const boardStyle = computed<CSSProperties>(() => ({
  width: `${boardWidth}px`,
  height: `${boardHeight}px`,
  zoom: scale.value,
}))
</script>

<template>
  <div
    ref="viewport"
    class="relative min-h-0 overflow-hidden rounded-z-lg bg-z-bg-layout bg-[radial-gradient(circle,color-mix(in_srgb,var(--z-color-text-quaternary)_24%,transparent)_1px,transparent_1px)] bg-size-[18px_18px] select-none"
    :class="dragging ? 'cursor-grabbing' : 'cursor-grab'"
    role="application"
    tabindex="0"
    aria-label="首页主题预览画板"
  >
    <div
      ref="stage"
      class="pointer-events-none absolute top-0 left-0 z-0 overflow-hidden"
      data-theme-preview-stage
      :data-scale="scalePercent"
      :style="stageStyle"
    >
      <ThemePreviewScope :theme="theme" :style="style" :styles-revision="stylesRevision">
        <div class="overflow-hidden bg-z-bg-layout p-6" data-theme-preview-board :style="boardStyle">
          <slot />
        </div>
      </ThemePreviewScope>
    </div>

    <div
      class="pointer-events-none absolute top-3 left-3 z-10 inline-flex items-center gap-2 rounded-z bg-z-bg-container/92 px-2.5 py-2 text-[10px] font-emphasis text-z-text-secondary shadow-z-tertiary backdrop-blur-sm"
    >
      <Move class="size-3.5 text-z-primary-text" />
      拖动空白处平移 · 滚轮缩放
    </div>

    <div
      class="absolute top-3 right-3 z-10 flex items-center gap-1 rounded-z-lg bg-z-bg-container/94 p-1.5 shadow-z-popup backdrop-blur-md"
      data-canvas-control
      @pointerdown.stop
      @dblclick.stop
      @wheel.stop
    >
      <ZIconButton aria-label="缩小画板" size="small" :disabled="!canZoomOut" @click="zoomOut">
        <Minus class="size-3.5" />
      </ZIconButton>
      <ZButton
        size="small" variant="text"
        class="min-w-14 px-2! font-mono tabular-nums"
        title="恢复 100%"
        @click="resetScale"
      >
        {{ scalePercent }}%
      </ZButton>
      <ZIconButton aria-label="放大画板" size="small" :disabled="!canZoomIn" @click="zoomIn">
        <Plus class="size-3.5" />
      </ZIconButton>
      <span class="mx-0.5 h-4 w-px bg-z-fill-secondary" />
      <ZIconButton aria-label="适应画板" size="small" @click="fitToViewport()">
        <Maximize2 class="size-3.5" />
      </ZIconButton>
    </div>
  </div>
</template>
