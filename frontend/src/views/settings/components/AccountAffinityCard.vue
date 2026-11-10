<script setup lang="ts">
import type { AccountAffinity } from '@/api'
import { ZCard, ZForm, ZFormItem, ZIconButton, ZInput, ZPopover, ZSegmented } from '@codex-proxy/ui'
import { CircleAlert } from '@lucide/vue'

const props = defineProps<{ disabled: boolean }>()
const accountAffinity = defineModel<AccountAffinity | ''>({ required: true })
const affinityTtlHours = defineModel<string>('ttlHours', { required: true })
const maxAccountRotations = defineModel<string>('maxAccountRotations', { required: true })
const recoveryAttempts = defineModel<string>('recoveryAttempts', { required: true })
const recoveryRotationAttempts = defineModel<string>('recoveryRotationAttempts', { required: true })
const affinityOptions = [
  { label: '宽松', value: 'relaxed' },
  { label: '优先', value: 'preferred' },
  { label: '严格', value: 'strict' },
]
</script>

<template>
  <ZCard title="亲和策略">
    <ZForm class="max-w-6xl sm:grid-cols-2 lg:grid-cols-3">
      <ZFormItem label="账号亲和" description="控制同一会话内的账号绑定方式">
        <template #label-extra>
          <ZPopover class="-my-1" trigger="hover-click" placement="top-start">
            <template #reference="{ open }">
              <ZIconButton aria-label="账号亲和说明" :title="undefined" :aria-expanded="open" class="size-6! hover:bg-transparent! active:bg-transparent!">
                <CircleAlert class="size-3.5" aria-hidden="true" />
              </ZIconButton>
            </template>
            <div class="max-w-72 space-y-2 px-3 py-2 text-z-sm leading-relaxed text-z-text-secondary">
              <p>宽松：会话内请求直接按调度策略选号，不优先主账号</p>
              <p>优先：会话内请求优先沿用主账号，繁忙或不可用时临时分流，不改变会话绑定</p>
              <p>严格：同一会话共用账号，子线程等待当前账号，并跟随主线程换号</p>
              <p>日常选号遵循调度策略，业务故障恢复按固定轮询执行</p>
            </div>
          </ZPopover>
        </template>
        <ZSegmented v-model="accountAffinity" aria-label="账号亲和" :options="affinityOptions" :disabled="props.disabled" class="w-full" />
      </ZFormItem>
      <ZFormItem label="亲和时长" description="账号绑定的保留时间，成功调度后续期">
        <ZInput v-model="affinityTtlHours" aria-label="亲和时长（小时）" type="number" min="1" max="720" step="1" :disabled="props.disabled">
          <template #suffix>
            小时
          </template>
        </ZInput>
      </ZFormItem>
      <ZFormItem label="其他恢复换号上限" description="连接、传输和续接等恢复路径的换号上限，0 表示禁止">
        <ZInput v-model="maxAccountRotations" aria-label="其他恢复换号上限" type="number" min="0" max="31" step="1" :disabled="props.disabled" />
      </ZFormItem>
      <ZFormItem label="初始账号尝试次数" description="包含首次请求，同账号失败后间隔 1 秒再试">
        <template #label-extra>
          <ZPopover class="-my-1" trigger="hover-click" placement="top-start">
            <template #reference="{ open }">
              <ZIconButton aria-label="初始账号尝试次数说明" :title="undefined" :aria-expanded="open" class="size-6! hover:bg-transparent! active:bg-transparent!">
                <CircleAlert class="size-3.5" aria-hidden="true" />
              </ZIconButton>
            </template>
            <div class="max-w-72 space-y-2 px-3 py-2 text-z-sm leading-relaxed text-z-text-secondary">
              <p>次数仅用于初始账号，不包含换号重试和最后一次保底</p>
              <p>初始账号 3 次、换号 0 次：A → A → A → 保底</p>
              <p>上游明确建议等待时优先使用其等待时间，账号不可用可提前结束本阶段</p>
              <p>成功或不可重试时提前结束，原生续接和安全重放限制仍然有效</p>
            </div>
          </ZPopover>
        </template>
        <ZInput v-model="recoveryAttempts" aria-label="初始账号尝试次数" type="number" min="1" max="31" step="1" :disabled="props.disabled" />
      </ZFormItem>
      <ZFormItem label="换号重试次数" description="其他普通账号合计尝试次数，0 表示直接保底">
        <template #label-extra>
          <ZPopover class="-my-1" trigger="hover-click" placement="top-start">
            <template #reference="{ open }">
              <ZIconButton aria-label="换号重试次数说明" :title="undefined" :aria-expanded="open" class="size-6! hover:bg-transparent! active:bg-transparent!">
                <CircleAlert class="size-3.5" aria-hidden="true" />
              </ZIconButton>
            </template>
            <div class="max-w-72 space-y-2 px-3 py-2 text-z-sm leading-relaxed text-z-text-secondary">
              <p>默认 1 次，换号后不重新获得初始账号的尝试次数</p>
              <p>初始账号 2 次、换号 1 次：A → A → B → 保底</p>
              <p>初始账号 3 次、换号 2 次：A → A → A → B → C → 保底</p>
              <p>只有 B 可用时可再次尝试 B，没有其他普通账号时直接保底</p>
              <p>初始账号尝试与换号重试合计最多 31 次，保底另算 1 次</p>
            </div>
          </ZPopover>
        </template>
        <ZInput v-model="recoveryRotationAttempts" aria-label="换号重试次数" type="number" min="0" max="30" step="1" :disabled="props.disabled" />
      </ZFormItem>
    </ZForm>
  </ZCard>
</template>
