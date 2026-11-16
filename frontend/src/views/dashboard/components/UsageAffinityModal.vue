<script setup lang="ts">
import type { UsageListRecord } from '@/api'
import { ZButton, ZDialog, ZFormItem, ZSelect } from '@codex-proxy/ui'
import { toRef } from 'vue'
import { useUsageAffinity } from '../composables/useUsageAffinity'

const props = defineProps<{ record: UsageListRecord | null }>()
const open = defineModel<boolean>({ default: false })
const {
  binding,
  targetAccount,
  operation,
  saveError,
  rejected,
  accountOptions,
  canSave,
  accountLabel,
  loading,
  error,
  accountsLoading,
  accountsError,
  saving,
  loadBinding,
  loadAccounts,
  save,
} = useUsageAffinity(toRef(props, 'record'), open)
</script>

<template>
  <ZDialog
    v-model="open"
    title="调整会话亲和"
    width="32rem"
    :show-close="!saving"
    :close-on-click-modal="!saving"
    :close-on-press-escape="!saving"
  >
    <p v-if="loading" class="m-0 text-z-sm text-z-text-secondary" role="status">
      正在加载此 Key 的亲和绑定
    </p>
    <div v-else-if="error" class="grid gap-3">
      <p class="m-0 text-z-sm text-z-error-text" role="alert">
        {{ error }}
      </p>
      <ZButton class="justify-self-start" @click="loadBinding">
        重新加载
      </ZButton>
    </div>
    <template v-else-if="binding">
      <dl class="mt-0 mb-4 grid gap-3 text-z-sm">
        <div class="grid grid-cols-[5rem_minmax(0,1fr)] gap-3">
          <dt class="text-z-text-secondary">
            上游 Key
          </dt>
          <dd class="m-0 wrap-anywhere text-z-text">
            {{ record?.callerTokenName || (record?.callerIdentity?.mode === 'newApi' ? record.callerIdentity.tokenId : '—') }}
          </dd>
        </div>
        <div class="grid grid-cols-[5rem_minmax(0,1fr)] gap-3">
          <dt class="text-z-text-secondary">
            当前主账号
          </dt>
          <dd class="m-0 wrap-anywhere text-z-text">
            {{ accountLabel(binding.accountId) }}
          </dd>
        </div>
      </dl>
      <p v-if="binding.directive" class="mb-4 text-z-sm text-z-orange-text" role="status">
        {{ binding.directive.action === 'target' ? `待切换至 ${accountLabel(binding.directive.accountId)}` : '此 Key 有待执行的账号调整' }}
      </p>
      <ZFormItem label="目标主账号" required>
        <ZSelect
          v-model="targetAccount"
          class="w-full"
          aria-label="目标主账号"
          placeholder="搜索账号名称、邮箱或 ID"
          filterable
          remote
          :loading="accountsLoading"
          :disabled="saving || !!operation"
          :options="accountOptions"
          @query-change="loadAccounts"
        />
        <div v-if="accountsError" class="mt-2 flex items-center gap-2 text-z-xs text-z-error-text" role="alert">
          {{ accountsError }}
          <ZButton size="small" :disabled="saving || !!operation" @click="loadAccounts()">
            重试
          </ZButton>
        </div>
      </ZFormItem>
      <p class="mb-0 text-z-sm text-z-text-secondary">
        仅调整此 Key 的主绑定，后续符合条件的新请求生效，已准入请求及原生续接保留原账号归属
      </p>
      <p v-if="saveError" class="mb-0 text-z-sm wrap-anywhere text-z-error-text" role="alert">
        {{ saveError }}
      </p>
    </template>
    <div v-else class="grid gap-3">
      <p class="m-0 text-z-sm text-z-text-secondary" role="status">
        此 Key 暂无可调整的主绑定，绑定可能尚未建立或已过期
      </p>
      <ZButton class="justify-self-start" @click="loadBinding">
        重新加载
      </ZButton>
    </div>
    <template #footer>
      <ZButton :disabled="saving" @click="open = false">
        取消
      </ZButton>
      <ZButton v-if="rejected || (saveError && !operation)" :disabled="saving" @click="loadBinding">
        重新加载绑定
      </ZButton>
      <ZButton v-else-if="binding" type="primary" :disabled="!canSave" :loading="saving" @click="save">
        {{ operation ? '重试保存' : '保存绑定' }}
      </ZButton>
    </template>
  </ZDialog>
</template>
