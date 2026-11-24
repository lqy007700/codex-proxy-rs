<script setup lang="ts">
import type { ProxyTargetResult } from '@/api'
import { ZPopover } from '@codex-proxy/ui'
import { computed } from 'vue'

const props = defineProps<{ name: string, results: ProxyTargetResult[] }>()
const custom = computed(() => props.results?.filter(result => !result.builtIn) ?? [])
const summary = computed(() => custom.value.length
  ? `目标 ${custom.value.filter(result => result.statusCode !== null).length}/${custom.value.length} 可达`
  : '测试详情')
</script>

<template>
  <ZPopover v-if="results?.length" placement="bottom-start">
    <template #reference="{ open }">
      <button type="button" class="cursor-pointer rounded-sm border-0 bg-transparent p-0 text-left text-z-xs whitespace-nowrap text-z-link hover:underline focus-visible:ring-2 focus-visible:ring-z-control-outline" :aria-label="`${name} 的测试详情，${summary}`" :aria-expanded="open">
        {{ summary }}
      </button>
    </template>
    <div class="w-88 max-w-[calc(100vw-2rem)] p-3">
      <div class="mb-2 text-z-sm font-medium text-z-text">
        {{ name }} · 测试详情
      </div>
      <ul class="m-0 list-none space-y-1 p-0">
        <li v-for="(result, index) in results" :key="index" class="rounded-z-sm px-2 py-1.5 odd:bg-z-fill-quaternary">
          <div class="flex items-baseline justify-between gap-2 text-z-xs">
            <span class="min-w-0 break-all font-medium text-z-text">{{ result.target.name }}</span>
            <span class="shrink-0 font-mono text-z-text-secondary">{{ result.latencyMs }} ms</span>
          </div>
          <div class="mt-0.5 break-all font-mono text-z-xs text-z-text-secondary">
            {{ result.target.url }}
          </div>
          <div class="mt-0.5 break-all text-z-xs" :class="result.error ? 'text-z-error' : 'text-z-success'">
            {{ result.error || (result.statusCode !== null ? `已响应 · HTTP ${result.statusCode}` : result.exitIp) }}
          </div>
        </li>
      </ul>
    </div>
  </ZPopover>
</template>
