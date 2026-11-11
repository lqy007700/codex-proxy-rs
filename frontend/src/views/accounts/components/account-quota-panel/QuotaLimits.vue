<script setup lang="ts">
import type { Account } from '@/api'
import { ZButton, ZDialog, ZFormItem, ZIconButton, ZInput } from '@codex-proxy/ui'
import { SlidersHorizontal } from '@lucide/vue'
import { computed, ref, shallowRef, watch } from 'vue'
import { batchUpdateAccounts, getAccountQuota } from '@/api'
import { useAsyncAction } from '@/composables/useAsyncAction'

const props = defineProps<{ account: Account }>()
const emit = defineEmits<{ accountUpdated: [account: Account] }>()
const open = shallowRef(false)
const draft = ref<Record<string, string>>({})
const { loading, run } = useAsyncAction()
const rows = computed(() => {
  const windows = new Map(props.account.quota.windows.filter(window => window.accountWide).map(window => [
    window.key,
    { key: window.key, label: window.labelDisplay, used: window.usedPercentDisplay },
  ]))
  for (const key of Object.keys(props.account.quotaLimits)) {
    if (!windows.has(key))
      windows.set(key, { key, label: key, used: '—' })
  }
  return Array.from(windows.values())
})
const invalid = computed(() => Object.values(draft.value).some(value => value !== '' && !/^(?:[1-9]\d?|100)$/.test(value)))

function edit() {
  draft.value = Object.fromEntries(Object.entries(props.account.quotaLimits).map(([key, value]) => [
    key,
    String(value),
  ]))
  open.value = true
}

async function save() {
  if (invalid.value)
    return
  const accountId = props.account.id
  const quotaLimits = Object.fromEntries(Object.entries(draft.value)
    .filter(([, value]) => value !== '')
    .map(([key, value]) => [
      key,
      Number(value),
    ]))
  await run(async () => {
    await batchUpdateAccounts({
      accountIds: [accountId],
      quotaLimits,
    })
    const { account } = await getAccountQuota({ accountId })
    if (props.account.id === accountId) {
      emit('accountUpdated', account)
      open.value = false
    }
  })
}

watch(() => props.account.id, () => {
  open.value = false
  draft.value = {}
})
</script>

<template>
  <ZIconButton size="small" aria-label="设置额度使用率上限" aria-haspopup="dialog" :pressed="open" @click="edit">
    <SlidersHorizontal class="size-3.5" />
  </ZIconButton>
  <ZDialog
    v-model="open" title="额度使用率上限" description="任一窗口达到上限时暂停新请求，在途请求继续，留空表示不限制"
    width="28rem" :close-on-click-modal="!loading" :close-on-press-escape="!loading" :show-close="!loading"
  >
    <div class="grid gap-3">
      <p v-if="!rows.length" class="m-0 text-z-sm text-z-text-secondary">
        暂无已观测的通用额度窗口，请先刷新额度
      </p>
      <ZFormItem v-for="row in rows" :key="row.key" :label="row.label">
        <ZInput v-model="draft[row.key]" type="number" min="1" max="100" step="1" :aria-label="row.label" placeholder="不限制" :disabled="loading">
          <template #suffix>
            %
          </template>
        </ZInput>
        <p class="mt-1 mb-0 text-z-xs" :class="account.quota.pausedWindows.includes(row.key) ? 'text-z-warning-text' : 'text-z-text-tertiary'">
          当前已用 {{ row.used }}
        </p>
      </ZFormItem>
      <p v-if="invalid" class="m-0 text-z-xs text-z-error-text" role="alert">
        请输入 1–100 的整数，或留空取消限制
      </p>
    </div>
    <template #footer>
      <ZButton :disabled="loading" @click="open = false">
        取消
      </ZButton>
      <ZButton type="primary" :disabled="!rows.length || invalid" :loading="loading" @click="save">
        保存
      </ZButton>
    </template>
  </ZDialog>
</template>
