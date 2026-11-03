<script setup lang="ts">
import type { CallerIdentityPolicy, IdentitySource } from '@/api/modules/affinity'
import { ZButton, ZFormItem, ZInput, ZSelect, ZSwitch } from '@codex-proxy/ui'
import { onMounted, ref, shallowRef } from 'vue'
import { getIdentitySources, saveIdentitySource } from '@/api/modules/affinity'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { generateRequestId } from '@/utils/operation'

const props = defineProps<{ disabled?: boolean }>()
const policy = defineModel<CallerIdentityPolicy>({ required: true })
const sources = shallowRef<IdentitySource[]>([])
const sourceName = ref('')
const error = shallowRef(false)
const action = useAsyncAction()
const { loading } = action
async function load() {
  await action.run(async () => {
    error.value = false
    sources.value = await getIdentitySources()
  }, {
    onError: () => error.value = true,
  })
}
function setMode(mode: string) {
  policy.value = mode === 'newApi'
    ? { mode, sourceId: sources.value.find(source => source.enabled)?.id ?? '', allowUnidentified: false }
    : { mode: 'direct' }
}
async function createSource() {
  if (!sourceName.value.trim())
    return
  await action.run(async () => {
    const source = {
      id: generateRequestId(),
      name: sourceName.value.trim(),
      enabled: true,
    }
    await saveIdentitySource(source)
    sources.value = await getIdentitySources()
    policy.value = { mode: 'newApi', sourceId: source.id, allowUnidentified: false }
    sourceName.value = ''
    error.value = false
  })
}
onMounted(load)
</script>

<template>
  <ZFormItem label="调用方身份">
    <ZSelect :model-value="policy.mode" :disabled="props.disabled" aria-label="调用方身份" :options="[{ label: '直连 CPR Key', value: 'direct' }, { label: 'New API 可信转发', value: 'newApi' }]" @update:model-value="setMode(String($event))" />
  </ZFormItem>
  <template v-if="policy.mode === 'newApi'">
    <ZFormItem label="接入来源" required>
      <ZSelect :model-value="policy.sourceId" :loading="loading" :disabled="props.disabled || loading" aria-label="接入来源" placeholder="选择 New API 来源" :options="sources.map(source => ({ label: source.enabled ? source.name : `${source.name}（已停用）`, value: source.id, disabled: !source.enabled }))" @update:model-value="policy = { ...policy, sourceId: String($event) }" />
      <p v-if="error" class="mt-2 text-cp-sm text-cp-error" role="alert">
        来源加载失败 <ZButton size="small" :loading="loading" @click="load">
          重试
        </ZButton>
      </p>
      <div class="mt-2 flex gap-2">
        <ZInput v-model="sourceName" :disabled="props.disabled || loading" aria-label="新来源名称" placeholder="新来源名称" />
        <ZButton :disabled="props.disabled || !sourceName.trim()" :loading="loading" @click="createSource">
          创建来源
        </ZButton>
      </div>
      <p class="mt-2 text-cp-xs text-cp-text-tertiary">
        同一来源可关联多个 CPR Key，轮换 Key 时复用此来源
      </p>
    </ZFormItem>
    <ZFormItem label="允许无身份请求">
      <ZSwitch :model-value="policy.allowUnidentified" :disabled="props.disabled" aria-label="允许无身份请求" @update:model-value="policy = { ...policy, allowUnidentified: $event }" />
      <p class="mt-2 text-cp-xs text-cp-text-tertiary">
        仅用于 New API 渠道探测，不建立用户亲和，也不能续接用户响应
      </p>
    </ZFormItem>
  </template>
</template>
