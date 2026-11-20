<script setup lang="ts">
import type { AccountModelTestPreview as Preview } from '../utils/modelTestPreview'
import type { AccountModelTestSummary } from '@/api'
import { useElementVisibility } from '@vueuse/core'
import { computed, shallowRef, useTemplateRef, watch } from 'vue'

import { getAccountModelTestDetail } from '@/api'
import { useRequestState } from '@/composables/useRequestState'
import { accountModelTestPreview } from '../utils/modelTestPreview'
import AccountModelTestPreview from './AccountModelTestPreview.vue'

const { record } = defineProps<{ record: AccountModelTestSummary }>()
const emit = defineEmits<{ enlarge: [preview: Preview] }>()
const container = useTemplateRef('container')
const visible = useElementVisibility(container)
const request = useRequestState()
const fetched = shallowRef(false)
const animation = shallowRef<Preview>()
const preview = computed(() => record.status === 'succeeded' && record.kind === 'image'
  ? accountModelTestPreview({ id: record.id, hasImage: true, text: null })
  : animation.value)

watch([
  visible,
  () => record.status,
], async ([inView, status]) => {
  if (!inView || status !== 'succeeded' || record.kind === 'image' || fetched.value)
    return

  fetched.value = true
  const requestId = request.start()
  try {
    const result = await getAccountModelTestDetail(record.id, { signal: request.signal, silent: true })
    if (request.isCurrent(requestId))
      animation.value = accountModelTestPreview(result)
  }
  catch {
    // 记录详情仍可单独打开，缩略图加载失败不打断列表
  }
  finally {
    request.finish(requestId)
  }
})
</script>

<template>
  <div ref="container" class="flex min-h-8 items-center">
    <div v-if="preview" class="relative h-[72px] w-32">
      <AccountModelTestPreview v-if="visible" :preview="preview" thumbnail />
      <button
        type="button"
        aria-label="放大预览"
        title="放大预览"
        class="absolute inset-0 cursor-zoom-in rounded-z border-0 bg-transparent p-0 focus-visible:ring-2 focus-visible:ring-z-control-outline"
        @click="emit('enlarge', preview)"
      />
    </div>
    <span v-else class="text-z-sm text-z-text-quaternary">—</span>
  </div>
</template>
