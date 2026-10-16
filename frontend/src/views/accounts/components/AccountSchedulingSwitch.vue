<script setup lang="ts">
import { ZSwitch } from '@codex-proxy/ui'
import { ref, watch } from 'vue'

const props = defineProps<{
  enabled: boolean
  pending: boolean
  accountLabel: string
}>()
const emit = defineEmits<{ toggle: [] }>()
const checked = ref(props.enabled)

// 原生开关先响应输入，保存结束后再以服务端状态校准，失败时也恢复键盘可读状态
watch(() => [props.enabled, props.pending], () => {
  if (!props.pending)
    checked.value = props.enabled
})
</script>

<template>
  <ZSwitch
    v-model="checked"
    :disabled="pending"
    :aria-busy="pending"
    :aria-label="`${accountLabel} 的账号调度`"
    :title="enabled ? '停用调度' : '启用调度'"
    @change="emit('toggle')"
  />
</template>
