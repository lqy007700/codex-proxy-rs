<script setup lang="ts">
import { ZFormItem, ZIconButton, ZInput } from '@codex-proxy/ui'
import { Dices } from '@lucide/vue'
import HelpPopover from '@/components/HelpPopover.vue'
import { generateRequestId as generateUuid } from '@/utils/operation'

defineProps<{ disabled: boolean, placeholder: string, error: string }>()
const seed = defineModel<string>({ required: true })
</script>

<template>
  <ZFormItem label="安装身份 Seed" :error="error">
    <template #label-extra>
      <HelpPopover label="安装身份 Seed" content="按 Key 与账号分别派生稳定的上游安装身份；修改已有 Key 的 Seed 可能影响旧会话续接" />
    </template>
    <template #extra>
      <ZIconButton
        variant="link"
        size="small"
        class="-my-2"
        aria-label="随机生成安装身份 Seed"
        :disabled="disabled"
        @click="seed = generateUuid()"
      >
        <Dices class="size-4" aria-hidden="true" />
      </ZIconButton>
    </template>
    <ZInput
      v-model="seed"
      aria-label="安装身份 Seed"
      :aria-invalid="!!error"
      :placeholder="placeholder"
      :disabled="disabled"
      :spellcheck="false"
      autocomplete="off"
      class="font-mono"
    />
  </ZFormItem>
</template>
