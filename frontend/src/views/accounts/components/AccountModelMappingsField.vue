<script setup lang="ts">
import type { AccountModelMappingRow } from '../utils/modelMappings'
import { ZButton, ZFormItem, ZIconButton, ZInput, ZPopover } from '@codex-proxy/ui'
import { CircleHelp, Plus, Trash2 } from '@lucide/vue'
import { useId } from 'vue'

defineProps<{
  disabled?: boolean
  models: string[]
}>()

const rows = defineModel<AccountModelMappingRow[]>({ required: true })
const catalogId = useId()
</script>

<template>
  <ZFormItem label="模型映射">
    <template #label-extra>
      <ZPopover trigger="click" placement="top-start" :teleported="false">
        <template #reference="{ open }">
          <ZIconButton size="small" variant="plain" aria-label="模型映射说明" :aria-expanded="open">
            <CircleHelp class="size-3.5" />
          </ZIconButton>
        </template>
        <div class="grid max-w-64 gap-2 p-3 text-cp-sm text-cp-text-secondary">
          <p class="m-0">
            优先使用本账号的映射，未命中时沿用全局映射
          </p>
          <p class="m-0">
            目标为实际发送的上游模型 ID，不继续套用其他映射，仍受模型配置和上游权限限制
          </p>
          <p class="m-0">
            仅作用于文本生成，清空后恢复全局规则
          </p>
        </div>
      </ZPopover>
    </template>
    <div class="grid gap-2">
      <div
        v-for="(row, index) in rows"
        :key="index"
        class="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-2 sm:grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)_auto]"
      >
        <ZInput v-model="row.requestedModel" placeholder="请求模型" :aria-label="`请求模型 ${index + 1}`" :disabled="disabled" />
        <span class="hidden text-cp-text-quaternary sm:block" aria-hidden="true">→</span>
        <ZInput
          v-model="row.upstreamModel"
          class="col-start-1 row-start-2 sm:col-start-auto sm:row-start-auto"
          placeholder="上游模型，可选择或输入"
          :aria-label="`上游模型 ${index + 1}`"
          :list="catalogId"
          :disabled="disabled"
        />
        <ZIconButton
          class="col-start-2 row-span-2 row-start-1 sm:col-start-auto sm:row-span-1 sm:row-start-auto"
          :aria-label="`删除模型映射 ${index + 1}`"
          :disabled="disabled"
          @click="rows.splice(index, 1)"
        >
          <Trash2 class="size-4 text-cp-error" />
        </ZIconButton>
      </div>
      <datalist :id="catalogId">
        <option v-for="id in models" :key="id" :value="id" />
      </datalist>
      <ZButton class="justify-self-start" size="small" :disabled="disabled || rows.length >= 256" @click="rows.push({ requestedModel: '', upstreamModel: '' })">
        <template #icon>
          <Plus class="size-3.5" />
        </template>
        添加映射
      </ZButton>
    </div>
  </ZFormItem>
</template>
