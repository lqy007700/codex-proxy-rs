<script setup lang="ts">
import type { ProxyTestTarget } from '@/api'
import { ZButton, ZDialog } from '@codex-proxy/ui'
import { Plus } from '@lucide/vue'
import { computed, ref, watch } from 'vue'
import { getProxyTestTargets, replaceProxyTestTargets } from '@/api'
import { ApiError } from '@/api/request'
import { useRequestState } from '@/composables/useRequestState'
import { notification } from '@/utils/notification'
import ProxyTestTargetRow from './ProxyTestTargetRow.vue'

const open = defineModel<boolean>({ required: true })
const maxTargets = 5
const request = useRequestState()
const { loading, error } = request
const saving = ref(false)
const stale = ref(false)
const revision = ref<number | null>(null)
const rows = ref<(ProxyTestTarget & { id: number })[]>([])
const invalid = computed(() => rows.value.some(row => !row.name.trim() || !row.url.trim()))
let nextId = 0

function add() {
  rows.value.push({ id: nextId++, name: '', url: '' })
}

async function load() {
  revision.value = null
  stale.value = false
  const id = request.start()
  try {
    const data = await getProxyTestTargets({ silent: true, signal: request.signal })
    if (request.isCurrent(id)) {
      revision.value = data.revision
      rows.value = data.targets.map(target => ({ ...target, id: nextId++ }))
    }
  }
  catch (cause) {
    request.fail(id, cause)
  }
  finally {
    request.finish(id)
  }
}

async function save() {
  if (revision.value === null || saving.value || invalid.value || stale.value)
    return
  const id = request.start()
  saving.value = true
  try {
    await replaceProxyTestTargets({
      revision: revision.value,
      targets: rows.value.map(row => ({
        name: row.name.trim(),
        url: row.url.trim(),
      })),
    }, {
      silent: true,
      retry: false,
      signal: request.signal,
    })
    if (request.isCurrent(id)) {
      notification.success({ message: '测试地址已保存' })
      open.value = false
    }
  }
  catch (cause) {
    request.fail(id, cause)
    stale.value = request.isCurrent(id) && cause instanceof ApiError && cause.status === 409
  }
  finally {
    saving.value = false
    request.finish(id)
  }
}

watch(open, (value) => {
  if (value)
    void load()
  else
    request.invalidate()
})
</script>

<template>
  <ZDialog v-model="open" title="测试地址" description="所有代理共用，每次测试最多附加 5 个公网 HTTPS 地址" width="40rem" :show-close="!saving" :close-on-click-modal="!saving" :close-on-press-escape="!saving">
    <div class="grid gap-3">
      <p v-if="error" class="m-0 text-z-sm text-z-error" role="alert">
        {{ error }}
      </p>
      <p v-if="revision === null && loading" class="m-0 text-z-sm text-z-text-secondary" role="status">
        正在读取测试地址
      </p>
      <ZButton v-if="!loading && (revision === null || stale)" class="justify-self-start" @click="load">
        重新读取
      </ZButton>
      <template v-if="revision !== null">
        <div v-if="rows.length" class="grid gap-2 text-z-xs text-z-text-secondary">
          <div class="grid grid-cols-[1fr_2fr_2rem] gap-2">
            <span>名称</span>
            <span>HTTPS URL</span>
          </div>
          <ProxyTestTargetRow
            v-for="(row, index) in rows" :key="row.id"
            v-model:name="row.name" v-model:url="row.url"
            :position="index + 1" :disabled="saving"
            @remove="rows.splice(index, 1)"
          />
        </div>
        <p v-else class="m-0 text-z-sm text-z-text-secondary">
          暂无附加地址，将测试内置 IPv4 和 IPv6 出口
        </p>
        <ZButton :disabled="rows.length >= maxTargets || saving" class="justify-self-start" @click="add">
          <Plus class="size-4" />
          添加地址
        </ZButton>
      </template>
    </div>
    <template #footer>
      <ZButton :disabled="saving" @click="open = false">
        取消
      </ZButton>
      <ZButton type="primary" :loading="saving" :disabled="revision === null || loading || invalid || stale" @click="save">
        保存
      </ZButton>
    </template>
  </ZDialog>
</template>
