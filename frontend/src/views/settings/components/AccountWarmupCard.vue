<script setup lang="ts">
import { ZCard, ZForm, ZFormItem, ZInput, ZSwitch } from '@codex-proxy/ui'
import { Clock, Sparkles } from '@lucide/vue'

const enabled = defineModel<boolean>('enabled', { required: true })
const cron = defineModel<string>('cron', { required: true })
const model = defineModel<string>('model', { required: true })
</script>

<template>
  <ZCard
    title="账号预激活"
    description="按部署时区向空闲的 Codex OAuth 账号发送一次请求，尝试启动额度窗口"
  >
    <ZForm class="max-w-6xl sm:grid-cols-2">
      <ZSwitch
        v-model="enabled"
        class="col-span-full justify-self-start"
        label="启用预激活"
      />

      <ZFormItem
        label="Cron 表达式"
        description="五段表达式：分 时 日 月 星期"
        :required="enabled"
      >
        <ZInput
          v-model="cron"
          :disabled="!enabled"
          aria-label="Cron 表达式"
          placeholder="0 8 * * *"
          maxlength="255"
          :spellcheck="false"
        >
          <template #prefix>
            <Clock class="size-4" />
          </template>
        </ZInput>
      </ZFormItem>

      <ZFormItem
        label="预激活模型"
        description="启用时必填，请填写账号可用的模型"
        :required="enabled"
      >
        <ZInput
          v-model="model"
          :disabled="!enabled"
          aria-label="预激活模型"
          placeholder="请输入模型名称"
        >
          <template #prefix>
            <Sparkles class="size-4" />
          </template>
        </ZInput>
      </ZFormItem>
    </ZForm>
  </ZCard>
</template>
