<script setup lang="ts">
import { ZPopover, ZTag } from '@codex-proxy/ui'
import { shallowRef, useTemplateRef } from 'vue'

defineProps<{
  label: string
  title: string
  disabled?: boolean
}>()

const selected = defineModel<boolean>({ required: true })
const labelText = useTemplateRef<HTMLSpanElement>('labelText')
const open = shallowRef(false)

function updateOpen(nextOpen: boolean) {
  const text = labelText.value
  open.value = nextOpen && Boolean(text && text.scrollWidth > text.clientWidth)
}

function dismissTooltip(event: KeyboardEvent, close: () => void) {
  if (!open.value)
    return
  event.preventDefault()
  event.stopPropagation()
  close()
}
</script>

<template>
  <ZPopover :model-value="open" class="min-w-0" trigger="hover" placement="top" role="tooltip" :hover-delay="240" @update:model-value="updateOpen">
    <template #reference="{ close }">
      <button
        type="button"
        class="rounded-z border-0 bg-transparent p-0 text-left outline-none transition-opacity duration-150 hover:opacity-85 focus-visible:ring-2 focus-visible:ring-z-control-outline focus-visible:ring-offset-2 focus-visible:ring-offset-z-bg-container disabled:cursor-not-allowed disabled:opacity-60 motion-reduce:transition-none"
        :disabled="disabled"
        :aria-pressed="selected"
        :title="title"
        @click="selected = !selected"
        @keydown.esc="dismissTooltip($event, close)"
      >
        <ZTag size="small" :type="selected ? 'primary' : 'default'" :disabled="disabled">
          <span ref="labelText" class="block max-w-48 truncate">{{ label }}</span>
        </ZTag>
      </button>
    </template>
    <div class="max-w-96 px-3 py-2 text-z-sm wrap-anywhere">
      {{ title }}
    </div>
  </ZPopover>
</template>
