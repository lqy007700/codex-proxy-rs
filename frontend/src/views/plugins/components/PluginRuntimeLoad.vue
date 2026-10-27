<script setup lang="ts">
import type { PluginInstanceRuntimeLoad } from '@/api'
import { ChevronDown } from '@lucide/vue'
import { computed } from 'vue'
import PluginHelpPopover from './PluginHelpPopover.vue'

const props = defineProps<{ load: PluginInstanceRuntimeLoad, revision: number | null }>()

const values = computed(() => [
  {
    label: '在途调用',
    value: props.load.activeCalls,
  },
  {
    label: '执行中',
    value: props.load.executingCalls,
  },
  {
    label: '排队中',
    value: props.load.queuedCalls,
  },
  {
    label: '等待恢复',
    value: props.load.resumingCalls,
  },
  {
    label: '驻留回调',
    value: props.load.pendingCallbacks,
  },
  {
    label: '活动流',
    value: props.load.activeStreams,
  },
])

const summary = computed(() => values.value.every(item => item.value === 0)
  ? '当前空闲'
  : `${props.load.activeCalls} 个在途调用`)
</script>

<template>
  <section class="flex items-start gap-2 text-cp-xs" aria-label="运行负载">
    <details class="group min-w-0 flex-1">
      <summary class="flex cursor-pointer list-none flex-wrap items-center justify-between gap-x-3 gap-y-1 rounded-cp py-1 text-cp-text-secondary outline-none hover:text-cp-text focus-visible:ring-2 focus-visible:ring-cp-control-outline active:text-cp-primary-text [&::-webkit-details-marker]:hidden">
        <span>运行负载</span>
        <span class="inline-flex items-center gap-2">
          {{ summary }}
          <ChevronDown class="size-3.5 shrink-0 transition-transform group-open:rotate-180 motion-reduce:transition-none" aria-hidden="true" />
        </span>
      </summary>
      <dl class="m-0 mt-2 grid grid-cols-2 gap-x-4 gap-y-2 sm:grid-cols-3">
        <div v-for="item in values" :key="item.label" class="flex min-w-0 items-baseline gap-2">
          <dt class="text-cp-text-secondary">
            {{ item.label }}
          </dt>
          <dd class="m-0 shrink-0 font-mono text-cp-text tabular-nums">
            {{ item.value }}
          </dd>
        </div>
      </dl>
    </details>
    <PluginHelpPopover label="运行负载说明" class="mt-0.5">
      <p v-if="revision !== null" class="m-0 font-emphasis text-cp-text">
        {{ `运行配置版本：${revision}` }}
      </p>
      <p class="m-0">
        当前运行配置的采样值，不含正在退出的旧配置，计数可能重叠
      </p>
    </PluginHelpPopover>
  </section>
</template>
