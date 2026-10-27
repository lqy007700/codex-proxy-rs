<script setup lang="ts">
import type { AlertSettingsForm } from '../../composables/useAlertSettings'

import { ZButton, ZCard, ZCheckbox, ZForm, ZFormItem, ZInput, ZSelect } from '@codex-proxy/ui'
import { Send } from '@lucide/vue'

defineProps<{
  disabled: boolean
  hasChanges: boolean
  testing: boolean
  passwordConfigured: boolean
}>()
defineEmits<{ test: [] }>()
const form = defineModel<AlertSettingsForm>({ required: true })
const securityOptions = [
  { label: 'TLS 加密连接', value: 'tls' },
  { label: 'STARTTLS 必需升级', value: 'start_tls' },
]
</script>

<template>
  <ZCard title="邮件配置">
    <template #actions>
      <ZButton :loading="testing" :disabled="disabled || hasChanges" :title="hasChanges ? '请先保存修改' : undefined" @click="$emit('test')">
        <template #icon>
          <Send class="size-4" />
        </template>
        发送测试邮件
      </ZButton>
    </template>
    <ZForm class="max-w-6xl sm:grid-cols-2">
      <ZFormItem label="SMTP 服务器" description="只填写主机名或 IPv4 地址">
        <ZInput v-model="form.smtpHost" aria-label="SMTP 服务器" placeholder="smtp.example.com" :disabled="disabled" maxlength="253" autocomplete="off" />
      </ZFormItem>
      <ZFormItem label="SMTP 端口">
        <ZInput v-model="form.smtpPort" aria-label="SMTP 端口" type="number" min="1" max="65535" :disabled="disabled" />
      </ZFormItem>
      <ZFormItem label="连接安全" description="验证服务器证书，不自动降级明文连接">
        <ZSelect v-model="form.smtpSecurity" aria-label="连接安全" :options="securityOptions" :disabled="disabled" />
      </ZFormItem>
      <ZFormItem label="SMTP 用户名" description="无需认证的邮件中继可以留空">
        <ZInput v-model="form.smtpUsername" aria-label="SMTP 用户名" :disabled="disabled" maxlength="320" autocomplete="off" />
      </ZFormItem>
      <ZFormItem label="SMTP 密码或授权码" :description="passwordConfigured ? '已保存，留空保留当前值' : '尚未配置'">
        <ZInput v-model="form.smtpPassword" aria-label="SMTP 密码或授权码" type="password" autocomplete="new-password" maxlength="4096" :disabled="disabled || form.clearPassword" />
      </ZFormItem>
      <div class="flex items-center">
        <ZCheckbox v-model="form.clearPassword" label="清除已保存的密码" :disabled="disabled || !passwordConfigured || !!form.smtpPassword" />
      </div>
      <ZFormItem label="发件人邮箱">
        <ZInput v-model="form.senderAddress" aria-label="发件人邮箱" type="email" placeholder="alerts@example.com" :disabled="disabled" maxlength="320" />
      </ZFormItem>
      <ZFormItem label="发件人名称">
        <ZInput v-model="form.senderName" aria-label="发件人名称" :disabled="disabled" maxlength="128" />
      </ZFormItem>
      <ZFormItem label="接收邮箱" description="多个地址用逗号分隔，最多 10 个" class="col-span-full">
        <ZInput v-model="form.recipients" aria-label="接收邮箱" placeholder="ops@example.com, admin@example.com" :disabled="disabled" maxlength="3210" />
      </ZFormItem>
    </ZForm>
    <p class="mt-4 text-cp-sm text-cp-text-secondary">
      测试使用已保存配置，每分钟最多一次，实际发送结果显示在投递记录中
    </p>
  </ZCard>
</template>
