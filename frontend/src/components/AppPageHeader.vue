<script setup lang="ts">
import { ZPageHeader } from '@codex-proxy/ui'

defineProps<{
  title: string
  description?: string
}>()
</script>

<template>
  <ZPageHeader
    :title="title"
    :description="description"
    class="min-h-13! [&>div>h1]:text-[24px] [&>div>p]:mt-1.5 [&>div>p]:text-z-sm"
  >
    <template v-if="$slots.description" #description>
      <slot name="description" />
    </template>
    <template v-if="$slots.actions" #actions>
      <slot name="actions" />
    </template>
  </ZPageHeader>
</template>
