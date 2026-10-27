<script setup lang="ts">
import type { Account } from '@/api'
import { ZIconButton, ZPopover, ZSwitch } from '@codex-proxy/ui'
import { CircleHelp } from '@lucide/vue'
import { shallowRef, watch } from 'vue'
import { batchUpdateAccounts, getAccountDetail } from '@/api'
import { useAsyncAction } from '@/composables/useAsyncAction'

const props = defineProps<{ account: Account }>()
const emit = defineEmits<{ accountUpdated: [account: Account] }>()
const enabled = shallowRef(false)
const saveAction = useAsyncAction()

watch(() => [props.account.id, props.account.allowCredits] as const, () => {
  enabled.value = props.account.allowCredits
}, { immediate: true })

async function toggle(allowCredits: boolean) {
  const accountId = props.account.id
  const previous = enabled.value
  enabled.value = allowCredits
  let saved = false
  await saveAction.run(async () => {
    await batchUpdateAccounts({
      accountIds: [accountId],
      allowCredits,
    })
    saved = true
    if (props.account.id !== accountId)
      return
    emit('accountUpdated', { ...props.account, allowCredits })
    const result = await getAccountDetail({ accountId })
    if (props.account.id === accountId)
      emit('accountUpdated', result.account)
  })
  if (!saved && props.account.id === accountId)
    enabled.value = previous
}
</script>

<template>
  <section class="grid gap-2 rounded-cp bg-cp-fill-quaternary px-4 py-3" aria-label="额度点数使用设置">
    <div class="flex flex-wrap items-center justify-between gap-2">
      <div class="flex items-center gap-1">
        <h3 class="m-0 text-cp-sm font-heavy text-cp-text">
          允许使用额度点数
        </h3>
        <ZPopover trigger="click" placement="bottom-start" role="dialog">
          <template #reference="{ open, id }">
            <ZIconButton size="small" class="size-6! p-0!" aria-label="额度点数使用规则" :aria-expanded="open" :aria-controls="id">
              <CircleHelp class="size-3.5" />
            </ZIconButton>
          </template>
          <div class="grid max-w-64 gap-2 p-3 text-cp-xs leading-relaxed text-cp-text-secondary">
            <p>默认关闭，套餐额度耗尽后暂停使用此账号</p>
            <p>开启后可继续使用上游可用点数，与自动使用重置卡独立</p>
            <p>额度信息可能延迟，已发出的请求仍可能消耗点数</p>
          </div>
        </ZPopover>
      </div>
      <ZSwitch
        class="credits-policy-switch"
        :model-value="enabled"
        :disabled="saveAction.loading.value"
        aria-label="允许使用额度点数"
        @update:model-value="toggle"
      />
    </div>
  </section>
</template>

<style scoped>
.credits-policy-switch :deep(input:not(:checked) + span) {
  background-color: var(--cp-switch-unchecked-bg);
}
</style>
