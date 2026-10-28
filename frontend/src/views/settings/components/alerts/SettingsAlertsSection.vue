<script setup lang="ts">
import { ZButton, ZEmpty, ZIconButton } from '@codex-proxy/ui'
import { Save, Undo2 } from '@lucide/vue'
import { watch } from 'vue'

import { useAlertSettings } from '../../composables/useAlertSettings'
import AlertMailCard from './AlertMailCard.vue'
import AlertRuleCard from './AlertRuleCard.vue'
import AlertStatusCards from './AlertStatusCards.vue'
import QuotaWarningCard from './QuotaWarningCard.vue'

const props = defineProps<{ active: boolean }>()
const {
  form,
  saved,
  status,
  preview,
  quotaPreview,
  hasChanges,
  busy,
  loading,
  error,
  statusLoading,
  statusError,
  saving,
  testing,
  previewing,
  start,
  stop,
  save,
  reset,
  refreshStatus,
  testEmail,
  previewWindow,
  previewQuota,
} = useAlertSettings()

defineExpose({ hasChanges })

watch(() => props.active, (active, _previous, onCleanup) => {
  if (active)
    void start()
  else
    stop()
  onCleanup(stop)
}, { immediate: true })
</script>

<template>
  <div class="grid w-full gap-5">
    <div class="flex flex-wrap items-center justify-end gap-2">
      <span v-if="hasChanges" class="mr-1 text-cp-sm text-cp-text-secondary">有未保存的修改</span>
      <ZIconButton v-if="hasChanges" aria-label="撤销告警配置更改" :disabled="busy" @click="reset">
        <Undo2 class="size-4" />
      </ZIconButton>
      <ZButton type="primary" :loading="saving" :disabled="busy || !hasChanges" @click="save">
        <template #icon>
          <Save class="size-4" />
        </template>
        保存告警配置
      </ZButton>
    </div>
    <ZEmpty v-if="error && !form" title="告警配置加载失败" role="alert">
      <template #action>
        <ZButton :loading="loading" @click="start">
          重新加载
        </ZButton>
      </template>
    </ZEmpty>
    <p v-else-if="!form" role="status" class="text-cp-sm text-cp-text-secondary">
      正在加载告警配置
    </p>
    <template v-else>
      <AlertRuleCard v-model="form" :disabled="busy" />
      <QuotaWarningCard
        v-model="form" :disabled="busy" :status="status?.quotaWarning ?? null"
        :enabled="saved?.quotaWarning?.enabled ?? false" :preview="quotaPreview"
        :previewing="previewing" :preview-disabled="busy || hasChanges" @preview="previewQuota"
      />
      <AlertMailCard v-model="form" :disabled="busy" :has-changes="hasChanges" :testing="testing" :password-configured="saved?.passwordConfigured ?? false" @test="testEmail" />
      <AlertStatusCards
        :status="status" :preview="preview" :loading="statusLoading" :error="statusError"
        :previewing="previewing" :action-disabled="busy || hasChanges"
        @refresh="refreshStatus" @preview="previewWindow"
      />
    </template>
  </div>
</template>
