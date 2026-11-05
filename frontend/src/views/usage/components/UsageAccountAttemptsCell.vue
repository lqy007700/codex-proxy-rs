<script setup lang="ts">
import type { UsageAttempt, UsageListRecord, UsageRecordDetail } from '@/api'
import { defineTableColumns, ZButton, ZPopover, ZTable } from '@codex-proxy/ui'
import { useEventListener } from '@vueuse/core'
import { computed, onScopeDispose, shallowRef, useTemplateRef, watch } from 'vue'
import { getUsageRecordDetail } from '@/api'
import { usageAccountText } from '@/components/usage/shared/presenter'
import { useRequestState } from '@/composables/useRequestState'
import { attemptOutcomeClass, attemptOutcomeText } from '../utils/detail'

const props = defineProps<{ record: UsageListRecord }>()
const open = shallowRef(false)
const accountButton = useTemplateRef<HTMLButtonElement>('accountButton')
const panel = useTemplateRef<HTMLElement>('panel')
const detail = shallowRef<UsageRecordDetail | null>(null)
const request = useRequestState()
const { loading, error } = request
const accountText = computed(() => usageAccountText(props.record))
const attempts = computed(() => detail.value?.attempts.slice().sort((a, b) => a.attemptIndex - b.attemptIndex) ?? [])
const incomplete = computed(() => detail.value && attempts.value.length < detail.value.attemptCount)
const columns = defineTableColumns<UsageAttempt>([
  {
    key: 'accountEmail',
    label: '账号',
    kind: 'custom',
    size: 'lg',
  },
  {
    key: 'providerErrorCode',
    label: '错误码',
    kind: 'custom',
    size: 'md',
  },
])
let closeTimer: ReturnType<typeof setTimeout> | undefined

function clearCloseTimer() {
  clearTimeout(closeTimer)
  closeTimer = undefined
}

function scheduleClose() {
  clearCloseTimer()
  if (open.value) {
    // 账号与传送到 body 的面板之间留出移动时间
    closeTimer = setTimeout(() => {
      open.value = false
    }, 200)
  }
}

async function loadAttempts() {
  if (!open.value)
    return
  detail.value = null
  const sequence = request.start()
  try {
    const result = await getUsageRecordDetail({ id: props.record.id }, { signal: request.signal, silent: true })
    if (request.isCurrent(sequence))
      detail.value = result
  }
  catch (cause) {
    request.fail(sequence, cause)
  }
  finally {
    request.finish(sequence)
  }
}

function attemptCode(attempt: UsageAttempt) {
  if (attempt.outcome === 'succeeded')
    return '成功'
  return attempt.providerErrorCode || attempt.statusCode || attemptOutcomeText(attempt.outcome)
}

watch(open, (value) => {
  clearCloseTimer()
  if (value)
    void loadAttempts()
  else
    request.invalidate({ resetLoading: false })
}, { flush: 'sync' })

watch([
  () => props.record.id,
  () => props.record.attemptCount,
], () => {
  open.value = false
  detail.value = null
  request.invalidate()
}, { flush: 'sync' })

onScopeDispose(clearCloseTimer)
useEventListener(accountButton, ['mouseenter', 'focusin'], clearCloseTimer)
useEventListener(accountButton, 'mouseleave', scheduleClose)
useEventListener(panel, ['mouseenter', 'focusin'], clearCloseTimer)
useEventListener(panel, 'mouseleave', scheduleClose)
</script>

<template>
  <ZPopover
    v-if="record.attemptCount > 1"
    v-model="open"
    class="min-w-0 max-w-full"
    trigger="click"
    placement="bottom-start"
  >
    <template #reference="{ id }">
      <button
        ref="accountButton"
        type="button"
        class="block max-w-full cursor-pointer truncate rounded-cp-sm border-0 bg-transparent p-0 text-left font-mono text-cp-sm leading-none font-bold text-cp-text outline-none hover:text-cp-primary-text focus-visible:ring-2 focus-visible:ring-cp-control-outline"
        :title="accountText"
        :aria-label="`${accountText}，查看请求尝试账号`"
        aria-haspopup="dialog"
        :aria-expanded="open"
        :aria-controls="open ? id : undefined"
      >
        {{ accountText }}
      </button>
    </template>

    <section
      ref="panel"
      class="w-96 max-w-[calc(100vw-2rem)] overflow-hidden rounded-cp-lg p-2"
      role="dialog"
      aria-label="请求尝试账号"
      :aria-busy="loading"
    >
      <p v-if="loading" class="m-0 px-2 py-3 text-cp-sm text-cp-text-secondary" role="status">
        正在加载尝试记录
      </p>
      <div v-else-if="error" class="grid gap-2 p-2">
        <p class="m-0 text-cp-sm wrap-break-word text-cp-error-text" role="alert">
          加载失败：{{ error }}
        </p>
        <ZButton size="small" class="justify-self-start" @click="loadAttempts">
          重试
        </ZButton>
      </div>
      <template v-else>
        <ZTable
          v-if="attempts.length"
          class="h-auto! max-h-64 min-w-0"
          :columns="columns"
          :data="attempts"
          size="small"
        >
          <template #accountEmail="{ row }">
            <span class="block font-mono text-cp-sm leading-5 wrap-anywhere whitespace-normal text-cp-text">
              {{ usageAccountText(row) }}
            </span>
          </template>
          <template #providerErrorCode="{ row }">
            <span class="block font-mono text-cp-sm leading-5 wrap-anywhere whitespace-normal" :class="attemptOutcomeClass(row.outcome)">
              {{ attemptCode(row) }}
            </span>
          </template>
        </ZTable>
        <p v-else class="m-0 px-2 py-3 text-cp-sm text-cp-text-secondary" role="status">
          暂无尝试记录
        </p>
        <p v-if="incomplete" class="mx-2 mt-2 mb-1 text-cp-xs text-cp-text-tertiary">
          尝试记录可能不完整
        </p>
      </template>
    </section>
  </ZPopover>
  <span
    v-else
    class="block max-w-full truncate font-mono text-cp-sm leading-none font-bold text-cp-text"
    :title="accountText"
  >
    {{ accountText }}
  </span>
</template>
