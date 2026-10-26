<script setup lang="ts">
import { ZConfigProvider } from '@codex-proxy/ui'
import { zhHans } from '@codex-proxy/ui/locale'
import { RouterView } from 'vue-router'
</script>

<template>
  <ZConfigProvider :locale="zhHans">
    <RouterView />
  </ZConfigProvider>
</template>
