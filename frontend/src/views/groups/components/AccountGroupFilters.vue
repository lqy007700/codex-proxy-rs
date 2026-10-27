<script setup lang="ts">
import { ZButton, ZInput } from '@codex-proxy/ui'

import { Plus, Search, Trash2 } from '@lucide/vue'

defineProps<{
  batchDeleting: boolean
  selectedCount: number
}>()
const emit = defineEmits<{
  create: []
  deleteSelected: []
}>()
const search = defineModel<string>('search', { required: true })
</script>

<template>
  <div class="flex w-full flex-col gap-3 sm:flex-row sm:items-center">
    <ZInput
      v-model="search"
      class="min-w-0 sm:w-80 sm:flex-none"
      aria-label="搜索账号分组"
      placeholder="搜索分组名称"
    >
      <template #prefix>
        <Search class="size-4.5 text-cp-text-tertiary" />
      </template>
    </ZInput>
    <div class="flex max-w-full shrink-0 flex-wrap items-center justify-end gap-2 sm:ml-auto">
      <ZButton
        v-if="selectedCount > 0"
        type="danger" variant="plain"
        :disabled="batchDeleting"
        @click="emit('deleteSelected')"
      >
        <template #icon>
          <Trash2 class="size-4" />
        </template>
        删除选中 ({{ selectedCount }})
      </ZButton>
      <ZButton type="primary" @click="emit('create')">
        <template #icon>
          <Plus class="size-4" />
        </template>
        创建分组
      </ZButton>
    </div>
  </div>
</template>
