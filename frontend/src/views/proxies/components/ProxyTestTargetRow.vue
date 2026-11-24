<script setup lang="ts">
import { ZIconButton, ZInput } from '@codex-proxy/ui'
import { Trash2 } from '@lucide/vue'

defineProps<{ position: number, disabled: boolean }>()
const emit = defineEmits<{ remove: [] }>()
const name = defineModel<string>('name', { required: true })
const url = defineModel<string>('url', { required: true })
</script>

<template>
  <div class="grid grid-cols-[1fr_2fr_2rem] items-center gap-2">
    <ZInput v-model="name" :disabled="disabled" :aria-label="`地址 ${position} 名称`" placeholder="OpenAI API" />
    <ZInput v-model="url" :disabled="disabled" :aria-label="`地址 ${position} URL`" placeholder="https://api.openai.com/v1/models" />
    <ZIconButton :disabled="disabled" :aria-label="`移除地址 ${position}`" @click="emit('remove')">
      <Trash2 class="size-4" />
    </ZIconButton>
  </div>
</template>
