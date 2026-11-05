<script setup lang="ts">
import type { BindingDirective } from '@/api/modules/affinity'
import { ZButton, ZMenuItem, ZPopover } from '@codex-proxy/ui'

defineProps<{
  token: boolean
  batch?: boolean
  disabled: boolean
  caller?: string
}>()

const emit = defineEmits<{
  action: [action: BindingDirective['action']]
}>()
</script>

<template>
  <div class="flex items-center gap-1">
    <ZButton
      :size="batch ? undefined : 'small'"
      :type="batch ? 'primary' : 'default'"
      :disabled="disabled"
      :aria-label="caller ? `设置 ${caller} 的${token ? '主账号' : '账号'}` : undefined"
      @click="emit('action', 'target')"
    >
      {{ batch ? token ? '批量指定主账号' : '批量指定账号' : token ? '设置主账号' : '指定账号' }}
    </ZButton>
    <ZPopover placement="bottom-end" :disabled="disabled">
      <template #reference="{ open }">
        <ZButton :size="batch ? undefined : 'small'" :disabled="disabled" :aria-expanded="open" :aria-label="caller ? `${caller} 的更多亲和操作` : '更多批量亲和操作'">
          更多
        </ZButton>
      </template>
      <template #default="{ close }">
        <div class="w-48 p-1.5">
          <ZMenuItem :disabled="disabled" @click="(close(), emit('action', 'reassign'))">
            {{ token ? '自动换一个账号' : '排除原账号重选' }}
          </ZMenuItem>
          <ZMenuItem :disabled="disabled" @click="(close(), emit('action', 'clear'))">
            {{ token ? '重置账号偏好' : '清除后重新选号' }}
          </ZMenuItem>
        </div>
      </template>
    </ZPopover>
  </div>
</template>
