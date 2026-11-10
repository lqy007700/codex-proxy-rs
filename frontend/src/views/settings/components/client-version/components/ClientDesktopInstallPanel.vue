<script setup lang="ts">
import type { CodexDesktopWindowsDownloads } from '@/api'
import { ExternalLink } from '@lucide/vue'

import ClientDesktopOfflineDownloads from './ClientDesktopOfflineDownloads.vue'
import ClientInstallCommandList from './ClientInstallCommandList.vue'

defineOptions({ name: 'ClientDesktopInstallPanel' })

withDefaults(
  defineProps<{
    downloads?: CodexDesktopWindowsDownloads | null
    loading?: boolean
    error?: string
  }>(),
  {
    downloads: null,
    loading: false,
    error: '',
  },
)

const emit = defineEmits<{
  refresh: []
  retry: []
}>()

const desktopCommands = [
  {
    label: 'Windows · Microsoft Store',
    command: 'winget install --id 9PLM9XGG6VKS -s msstore',
  },
  {
    label: 'Ubuntu / Debian · 升级',
    command: 'sudo apt update && sudo apt install --only-upgrade chatgpt',
  },
  {
    label: 'Fedora / RHEL · 升级',
    command: 'sudo dnf upgrade --refresh chatgpt',
  },
]
</script>

<template>
  <section class="grid gap-4" aria-labelledby="codex-desktop-online-title">
    <div class="grid gap-3 rounded-z-card bg-z-fill-quaternary p-4">
      <div class="flex flex-wrap items-start justify-between gap-3">
        <div class="min-w-0">
          <h3 id="codex-desktop-online-title" class="m-0 text-z-sm font-heavy text-z-text">
            在线安装与升级
          </h3>
          <p class="mt-1.5 mb-0 text-z-sm leading-[1.45] font-semibold text-z-text-secondary">
            桌面应用通常会自动更新，也可以从官方渠道重新安装
          </p>
        </div>
        <a
          href="https://chatgpt.com/download/"
          target="_blank"
          rel="noopener noreferrer"
          class="inline-flex h-z-control shrink-0 items-center gap-2 rounded-z-sm bg-z-fill-tertiary px-3.5 text-z font-bold text-z-text shadow-z-tertiary transition-[background-color,transform] hover:bg-z-bg-text-active motion-safe:active:translate-y-px"
        >
          官方下载页
          <ExternalLink class="size-3.5" />
        </a>
      </div>
      <ClientInstallCommandList :commands="desktopCommands" />
    </div>

    <ClientDesktopOfflineDownloads
      :downloads="downloads"
      :loading="loading"
      :error="error"
      @retry="emit('retry')"
      @refresh="emit('refresh')"
    />
  </section>
</template>
