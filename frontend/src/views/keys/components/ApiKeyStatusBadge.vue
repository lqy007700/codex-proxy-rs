<script setup lang="ts">
defineProps<{
  apiKey: {
    enabled: boolean
  }
}>()
</script>

<template>
  <span
    class="inline-flex h-6 min-w-14 items-center justify-center rounded-full px-2 text-z-sm leading-none font-bold"
    :class="{
      'bg-z-success-container text-z-success-on-container': apiKey.enabled,
      'bg-z-fill-quaternary text-z-text-secondary': !apiKey.enabled,
    }"
  >
    {{ apiKey.enabled ? '已启用' : '已禁用' }}
  </span>
</template>
