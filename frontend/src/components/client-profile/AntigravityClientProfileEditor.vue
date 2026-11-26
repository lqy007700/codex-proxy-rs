<script setup lang="ts">
import type {
  AntigravityClientProfileOptions,
  AntigravityClientProfilePreview,
  AntigravityClientProfileSelection,
} from '@/api/modules/settings/profiles'
import { ZButton, ZEmpty, ZFormItem, ZInput, ZSegmented, ZSelect, ZTextarea } from '@codex-proxy/ui'
import { computed, onMounted, shallowRef, watch } from 'vue'
import {
  getAntigravityClientProfileOptions,
  previewAntigravityClientProfile,
} from '@/api/modules/settings/profiles'
import { errorMessage } from '@/utils/operation'
import ClientProfilePreviewPanel from './ClientProfilePreviewPanel.vue'

const props = withDefaults(defineProps<{ active?: boolean, disabled?: boolean, allowInherit?: boolean, allowPreserve?: boolean }>(), {
  active: true,
  disabled: false,
  allowInherit: false,
  allowPreserve: false,
})
const model = defineModel<AntigravityClientProfileSelection | null | undefined>({ required: true })
const options = shallowRef<AntigravityClientProfileOptions>()
const preview = shallowRef<AntigravityClientProfilePreview>()
const loadError = shallowRef('')
const previewError = shallowRef('')
const loading = shallowRef(false)
const previewing = shallowRef(false)
const needsVersionInput = computed(() => model.value?.versionMode === 'fixed' && !model.value.clientVersion)

const platforms = { macos: 'MacOS', linux: 'Linux', windows: 'Windows' } as const
const presetOptions = computed(() => (options.value?.presets ?? []).map(({ configuration }) => ({
  label: `${platforms[configuration.platform]} · ${configuration.client === 'desktop'
    ? 'Desktop'
    : 'CLI'}`,
  value: `${configuration.platform}-${configuration.client}`,
})))
const currentPreset = computed(() => options.value?.presets.find(({ configuration }) =>
  configuration.platform === model.value?.platform && configuration.client === model.value?.client))

const source = computed({
  get: () => model.value === undefined
    ? 'keep'
    : model.value === null
      ? 'global'
      : 'independent',
  set: (value: string) => {
    if (value === 'keep') {
      model.value = undefined
      return
    }
    if (value === 'global')
      model.value = null
    else if (options.value?.globalConfiguration)
      model.value = { ...options.value.globalConfiguration }
  },
})

const selectedPreset = computed({
  get: () => model.value
    ? `${model.value.platform}-${model.value.client}`
    : undefined,
  set: (value: string) => {
    const preset = options.value?.presets.find(({ configuration }) => `${configuration.platform}-${configuration.client}` === value)
    if (preset) {
      model.value = {
        ...preset.configuration,
        systemPrompt: model.value?.systemPrompt ?? null,
        versionMode: preset.automaticAvailable ? 'latest' : 'fixed',
      }
    }
  },
})

const versionMode = computed({
  get: () => model.value?.versionMode ?? 'latest',
  set: (value: string) => {
    if (model.value) {
      model.value = {
        ...model.value,
        versionMode: value === 'fixed'
          ? 'fixed'
          : 'latest',
        clientVersion: value === 'fixed'
          ? preview.value?.clientVersion ?? null
          : null,
      }
    }
  },
})

const systemPrompt = computed({
  get: () => model.value?.systemPrompt ?? options.value?.defaultSystemPrompt ?? '',
  set: (value: string) => updateSystemPrompt(value),
})

function updateVersion(value: string) {
  if (model.value)
    model.value = { ...model.value, clientVersion: value.trim() || null }
}

function updateSystemPrompt(value: string | null) {
  if (model.value)
    model.value = { ...model.value, systemPrompt: value }
}

async function load() {
  loading.value = true
  loadError.value = ''
  try {
    options.value = await getAntigravityClientProfileOptions()
  }
  catch (error) {
    loadError.value = errorMessage(error)
  }
  finally {
    loading.value = false
  }
}

watch([
  model,
  () => props.active,
], ([configuration, active], _, onCleanup) => {
  if (!active || configuration === undefined)
    return
  let cancelled = false
  preview.value = undefined
  previewError.value = ''
  previewing.value = !needsVersionInput.value
  if (needsVersionInput.value)
    return
  const timer = setTimeout(async () => {
    try {
      const result = await previewAntigravityClientProfile({ configuration })
      if (!cancelled)
        preview.value = result
    }
    catch (error) {
      if (!cancelled)
        previewError.value = errorMessage(error)
    }
    finally {
      if (!cancelled)
        previewing.value = false
    }
  }, 250)
  onCleanup(() => {
    cancelled = true
    clearTimeout(timer)
  })
}, { immediate: true })

onMounted(() => void load())
</script>

<template>
  <div class="grid min-w-0 gap-4">
    <div v-if="allowInherit" class="flex flex-wrap items-center justify-between gap-3">
      <ZSegmented
        v-model="source"
        aria-label="Antigravity 客户端身份来源"
        class="shrink-0"
        :options="[
          ...(allowPreserve ? [{ label: '保持原值', value: 'keep' }] : []),
          { label: '全局配置', value: 'global' },
          { label: '独立配置', value: 'independent' },
        ]"
        :disabled="disabled || !options?.globalConfiguration"
      />
      <slot name="source-extra" />
    </div>
    <ZEmpty v-if="loadError" title="全局配置加载失败" :description="loadError" size="small" surface="none" role="alert">
      <template #action>
        <ZButton size="small" @click="load">
          重试
        </ZButton>
      </template>
    </ZEmpty>
    <template v-if="model">
      <div class="grid gap-4 sm:grid-cols-2">
        <ZFormItem label="客户端预设">
          <ZSelect v-model="selectedPreset" class="w-full" :options="presetOptions" :disabled="disabled || loading || !presetOptions.length" />
        </ZFormItem>
        <ZFormItem label="版本策略">
          <ZSelect
            v-model="versionMode"
            class="w-full"
            :disabled="disabled || loading"
            :options="[
              { label: '跟随最新版本', value: 'latest', disabled: !currentPreset?.automaticAvailable },
              { label: '自定义版本', value: 'fixed' },
            ]"
          />
        </ZFormItem>
      </div>
      <p v-if="currentPreset?.reason" class="m-0 text-z-sm text-z-text-secondary">
        {{ currentPreset.reason }}
      </p>
      <div v-if="model.versionMode === 'fixed'" class="grid gap-4 sm:grid-cols-2">
        <ZFormItem label="客户端版本" required>
          <ZInput
            :model-value="model.clientVersion ?? ''"
            :disabled="disabled || loading"
            placeholder="例如：2.34.0"
            @update:model-value="updateVersion($event)"
          />
        </ZFormItem>
      </div>
      <ZFormItem label="系统提示词">
        <div class="grid gap-2">
          <ZTextarea
            v-model="systemPrompt"
            aria-label="系统提示词"
            :disabled="disabled || loading"
            :rows="5"
          />
          <div class="flex flex-wrap items-center justify-between gap-2 text-z-xs text-z-text-secondary">
            <span v-if="options?.systemPromptSource">默认来源：{{ options.systemPromptSource }}</span>
            <ZButton variant="link" size="small" :disabled="disabled || loading || model.systemPrompt == null" @click="updateSystemPrompt(null)">
              恢复默认系统提示词
            </ZButton>
          </div>
        </div>
      </ZFormItem>
    </template>
    <ClientProfilePreviewPanel v-if="model !== undefined && !loadError" :preview="preview" :previewing="previewing" :needs-version-input="needsVersionInput" :error="previewError" />
  </div>
</template>
