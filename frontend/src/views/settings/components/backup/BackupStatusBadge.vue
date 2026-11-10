<script setup lang="ts">
import type { BackupStatus } from '@/api'

import { computed } from 'vue'

const props = withDefaults(
  defineProps<{
    status: BackupStatus
    variant?: 'inline' | 'pill'
  }>(),
  {
    variant: 'inline',
  },
)

const STATUS_LABELS: Record<BackupStatus, string> = {
  queued: '排队中',
  dumping: '导出中',
  uploading: '上传中',
  completed: '已完成',
  failed: '失败',
  deleting: '删除中',
}

const STATUS_TONES: Record<BackupStatus, 'success' | 'danger' | 'warning' | 'info' | 'neutral'> = {
  queued: 'info',
  dumping: 'info',
  uploading: 'info',
  completed: 'success',
  failed: 'danger',
  deleting: 'warning',
}

const tone = computed(() => STATUS_TONES[props.status])
const label = computed(() => STATUS_LABELS[props.status] || props.status)

const textClass = computed(() => {
  switch (tone.value) {
    case 'success':
      return 'text-z-success-text'
    case 'danger':
      return 'text-z-error-text'
    case 'warning':
      return 'text-z-warning-text'
    case 'info':
      return 'text-z-info-text'
    default:
      return 'text-z-text-secondary'
  }
})

const dotClass = computed(() => {
  switch (tone.value) {
    case 'success':
      return 'bg-z-success'
    case 'danger':
      return 'bg-z-error'
    case 'warning':
      return 'bg-z-warning'
    case 'info':
      return 'bg-z-info'
    default:
      return 'bg-z-text-quaternary'
  }
})
</script>

<template>
  <span
    v-if="variant === 'pill'"
    class="inline-flex h-7 shrink-0 items-center rounded-full px-2.5 text-z-sm font-heavy"
    :class="textClass"
  >
    {{ label }}
  </span>
  <span
    v-else
    class="inline-flex min-w-16 items-center gap-1.5 text-z-sm leading-none font-emphasis"
    :class="textClass"
  >
    <span class="size-1.5 rounded-full" :class="dotClass" />
    <span>{{ label }}</span>
  </span>
</template>
