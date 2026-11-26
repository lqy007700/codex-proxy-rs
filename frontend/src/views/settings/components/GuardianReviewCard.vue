<script setup lang="ts">
import { ZCard, ZFormItem, ZSwitch } from '@codex-proxy/ui'
import HelpPopover from '@/components/HelpPopover.vue'

defineProps<{ disabled: boolean }>()
const enabled = defineModel<boolean>({ required: true })
</script>

<template>
  <ZCard title="自动审批审核" description="配置 Codex 同步审核请求的专用模型路由">
    <div class="max-w-6xl">
      <ZFormItem label="专用审核模型" description="同步审核优先使用授权账号的专用模型">
        <template #label-extra>
          <HelpPopover
            label="专用审核模型说明"
            content="开启时优先使用 codex-auto-review，关闭此项或没有可用账号支持该模型时，沿用客户端请求的模型"
          />
        </template>
        <ZSwitch
          v-model="enabled"
          aria-label="专用审核模型"
          active-text="开启"
          inactive-text="关闭"
          inline-prompt
          :width="56"
          :disabled="disabled"
        />
      </ZFormItem>
    </div>
  </ZCard>
</template>
