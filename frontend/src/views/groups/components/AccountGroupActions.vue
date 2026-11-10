<script setup lang="ts">
import type { AccountGroup } from '@/api'
import { ZIconButton } from '@codex-proxy/ui'

import { Pencil, Power, Trash2 } from '@lucide/vue'

defineProps<{
  group: AccountGroup
  updatingStatus: boolean
  deleting: boolean
}>()
const emit = defineEmits<{
  edit: [group: AccountGroup]
  toggle: [group: AccountGroup]
  delete: [group: AccountGroup]
}>()
</script>

<template>
  <div class="flex items-center gap-1">
    <ZIconButton
      size="small"
      aria-label="编辑分组"
      @click.stop="emit('edit', group)"
    >
      <Pencil class="size-3.5 text-z-link" />
    </ZIconButton>
    <ZIconButton
      size="small"
      :aria-label="group.enabled ? '禁用分组' : '启用分组'"
      :loading="updatingStatus"
      @click.stop="emit('toggle', group)"
    >
      <Power
        class="size-3.5"
        :class="group.enabled ? 'text-z-warning' : 'text-z-success'"
      />
    </ZIconButton>
    <ZIconButton
      size="small"
      aria-label="删除分组"
      :disabled="deleting"
      @click.stop="emit('delete', group)"
    >
      <Trash2 class="size-3.5 text-z-error" />
    </ZIconButton>
  </div>
</template>
