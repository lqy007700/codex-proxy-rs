<script setup lang="ts">
import type { SystemUpdateChannel, SystemUpdateDetail } from '@/api'
import { ZButton, ZConfirmDialog, ZDialog, ZForm, ZFormItem, ZMarkdown, ZScrollbar, ZSelect } from '@codex-proxy/ui'
import { ExternalLink } from '@lucide/vue'
import { computed, shallowRef, watch } from 'vue'

const props = defineProps<{
  info: SystemUpdateDetail | null
  currentVersion: string
  error: string
  loading: boolean
  canInstall: boolean
}>()
const emit = defineEmits<{
  refresh: []
  confirm: [version: string, channel: SystemUpdateChannel]
}>()
const open = defineModel<boolean>({ required: true })
const selected = shallowRef<string>()
const confirmOpen = shallowRef(false)
const checkError = computed(() => props.error || props.info?.warning)
const candidates = computed(() => props.info?.downgradeVersions ?? [])
const options = computed(() => candidates.value.map(candidate => ({
  value: candidate.version,
  label: `v${candidate.version}`,
  description: candidate.unsupportedReason ?? undefined,
  disabled: Boolean(candidate.unsupportedReason),
})))
const target = computed(() => candidates.value.find(candidate => candidate.version === selected.value))
const canConfirm = computed(() => props.canInstall && !props.loading && Boolean(target.value) && !target.value?.unsupportedReason)

watch(open, () => {
  selected.value = undefined
  confirmOpen.value = false
})
watch(() => props.info, () => {
  confirmOpen.value = false
  if (!target.value || target.value.unsupportedReason)
    selected.value = undefined
})

function handleConfirm() {
  if (!canConfirm.value || !target.value || !props.info)
    return
  confirmOpen.value = false
  emit('confirm', target.value.version, props.info.policy.channel)
}
</script>

<template>
  <ZDialog v-model="open" title="版本降级" description="仅可选择当前版本之前、所选通道内的最近 5 个版本" type="warning" width="36rem">
    <div class="grid gap-4">
      <div role="note" class="grid gap-1.5 rounded-z bg-z-warning-container px-3 py-3 text-z-sm leading-relaxed text-z-warning-on-container">
        <strong class="font-heavy">降级可能导致服务无法启动或部分功能不可用</strong>
        <span>仅替换程序文件，数据库与配置不会回退，请先备份并确认旧版兼容性</span>
      </div>
      <ZForm class="grid gap-4">
        <ZFormItem label="当前运行版本">
          <p class="m-0 wrap-anywhere font-mono text-z-sm text-z-text">
            v{{ currentVersion }}
          </p>
        </ZFormItem>
        <ZFormItem label="目标版本" required>
          <ZSelect
            v-model="selected"
            :options="options"
            :disabled="loading || !canInstall"
            :loading="loading"
            :placeholder="loading ? '正在检查历史版本' : '请选择降级目标'"
            aria-label="降级目标版本"
            class="w-full"
          />
        </ZFormItem>
      </ZForm>
      <p v-if="checkError" role="alert" class="m-0 text-z-sm text-z-error">
        {{ checkError }}
      </p>
      <p v-else-if="!loading && !candidates.length" role="status" class="m-0 text-z-sm text-z-text-secondary">
        当前通道没有符合条件的历史版本
      </p>
      <section v-if="target" class="grid gap-2">
        <div class="flex items-center justify-between gap-3">
          <strong class="text-z-sm font-heavy text-z-text">目标版本发布说明</strong>
          <a v-if="target.releaseUrl" :href="target.releaseUrl" target="_blank" rel="noreferrer" class="inline-flex items-center gap-1 text-z-xs text-z-link hover:text-z-link-hover">
            发布页 <ExternalLink class="size-3" />
          </a>
        </div>
        <ZScrollbar max-height="180px">
          <ZMarkdown v-if="target.notes?.trim()" :source="target.notes" />
          <p v-else class="m-0 text-z-sm text-z-text-secondary">
            此版本未提供发布说明
          </p>
        </ZScrollbar>
      </section>
    </div>
    <template #footer>
      <ZButton :loading="loading" :disabled="confirmOpen" class="mr-auto" @click="emit('refresh')">
        重新检查
      </ZButton>
      <ZButton @click="open = false">
        取消
      </ZButton>
      <ZButton type="primary" :disabled="!canConfirm" @click="confirmOpen = true">
        下载并降级
      </ZButton>
    </template>
  </ZDialog>

  <ZConfirmDialog
    v-model="confirmOpen"
    title="确认版本降级"
    description="降级完成后仍需重启服务才能生效"
    confirm-text="确认降级"
    destructive
    :confirm-disabled="!open || !canConfirm"
    @confirm="handleConfirm"
  >
    <div class="grid gap-4 text-z-sm">
      <dl class="m-0 grid gap-3 rounded-z bg-z-fill-quaternary p-3">
        <div class="grid gap-1">
          <dt class="text-z-text-quaternary">
            当前版本
          </dt>
          <dd class="m-0 wrap-anywhere font-mono text-z-text">
            v{{ currentVersion }}
          </dd>
        </div>
        <div class="grid gap-1">
          <dt class="text-z-text-quaternary">
            降级目标
          </dt>
          <dd class="m-0 wrap-anywhere font-mono text-z-text">
            v{{ target?.version }}
          </dd>
        </div>
      </dl>
      <p class="m-0 leading-relaxed text-z-error">
        数据库与配置不会回退，旧版可能无法读取现有数据或启动服务，请确认已备份并了解兼容性风险
      </p>
    </div>
  </ZConfirmDialog>
</template>
