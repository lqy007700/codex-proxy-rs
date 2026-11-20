<script setup lang="ts">
import type { Account, AccountModelTestDetail, AccountModelTestKind } from '@/api'
import { ZButton, ZDialog, ZFormItem, ZIconButton, ZInput, ZSelect, ZTextarea } from '@codex-proxy/ui'
import { RefreshCw } from '@lucide/vue'
import { useIntervalFn } from '@vueuse/core'
import { computed, ref, shallowRef, watch } from 'vue'

import { getAccountModels, getAccountModelTestDetail, refreshAccountModels, startAccountModelTest } from '@/api'
import { useRequestState } from '@/composables/useRequestState'
import { errorMessage } from '@/utils/operation'
import AccountIdentityCell from './AccountIdentityCell.vue'
import AccountModelTestResult from './AccountModelTestResult.vue'

const props = defineProps<{ account: Account, seed?: AccountModelTestDetail }>()
const emit = defineEmits<{ history: [account: Account], started: [] }>()
const open = defineModel<boolean>({ required: true })

const templates: { value: string, label: string, kind: AccountModelTestKind, prompt: string }[] = [
  {
    value: 'blank',
    label: '空白题目',
    kind: 'text',
    prompt: '',
  },
  {
    value: 'logic',
    label: '逻辑排序',
    kind: 'text',
    prompt: 'A、B、C、D 排成一列。A 在 C 前面，B 紧挨在 D 前面，C 不能排最后。列出所有满足条件的排列，并解释为什么没有其他排列。',
  },
  {
    value: 'json',
    label: '格式约束',
    kind: 'text',
    prompt: '返回一个 JSON 数组，包含三个对象。每个对象只有 name 和 score 两个键，score 分别为 8、13、21。第三个名字由前两个名字的首字组成。不要使用 Markdown。',
  },
  {
    value: 'poster',
    label: '图片 · 海报构图',
    kind: 'image',
    prompt: '生成一张露营海报。月亮在左上角，帐篷在右下角，标题写「山间一夜」，画面只使用蓝色和橙色。',
  },
  {
    value: 'svg',
    label: '动画 · 循环球',
    kind: 'animation',
    prompt: '输出一个独立的 SVG。画布为 300×160，蓝色小球在黄色平台上方循环跳动，用 SVG 动画实现。不要引用外部资源或脚本，只输出 SVG 源码。',
  },
]
const kinds = [
  { value: 'text', label: '文本' },
  { value: 'image', label: '原生图片' },
  { value: 'animation', label: 'SVG 动画' },
] as const

const templateId = ref('blank')
const kind = ref<AccountModelTestKind>('text')
const prompt = ref('')
const modelId = ref<string>()
const imageModelId = ref('')
const selectedModelId = computed(() => (kind.value === 'image'
  ? imageModelId.value
  : modelId.value || '').trim())
const modelOptions = shallowRef<{ label: string, value: string }[]>([])
const models = useRequestState()
const detailRequest = useRequestState()
const submitting = ref(false)
const submitError = ref('')
const testId = ref('')
const result = shallowRef<AccountModelTestDetail>()
const running = computed(() => Boolean(testId.value) && (!result.value || result.value.status === 'running'))
const canGenerateImage = computed(() => props.account.provider === 'openai'
  && !(props.account.authenticationKind === 'oauth' && props.account.planType?.trim().toLowerCase() === 'free'))
const visibleTemplates = computed(() => templates.filter(item => item.kind !== 'image' || canGenerateImage.value))
const visibleKinds = computed(() => kinds.filter(item => item.value !== 'image' || canGenerateImage.value))
const promptTooLong = computed(() => new TextEncoder().encode(prompt.value).length > 32768)
const canSubmit = computed(() => !submitting.value && !running.value && (kind.value === 'image' || !models.loading.value)
  && Boolean(selectedModelId.value && prompt.value.trim())
  && !promptTooLong.value && (kind.value !== 'image' || canGenerateImage.value))

function applyTemplate(value = templateId.value) {
  const item = templates.find(item => item.value === value)
  if (!item)
    return
  templateId.value = item.value
  kind.value = item.kind
  prompt.value = item.prompt
}

async function loadModels(refresh = false) {
  const requestId = models.start()
  try {
    const response = await (refresh ? refreshAccountModels : getAccountModels)(
      { accountId: props.account.id },
      { signal: models.signal, silent: true },
    )
    if (!models.isCurrent(requestId))
      return
    const ids = new Set(response.models.map(model => model.id))
    if (modelId.value)
      ids.add(modelId.value)
    modelOptions.value = [...ids].map(value => ({ value, label: value }))
  }
  catch (cause) {
    models.fail(requestId, cause)
  }
  finally {
    models.finish(requestId)
  }
}

async function loadResult(background = false) {
  if (!testId.value || detailRequest.loading.value)
    return
  const requestId = detailRequest.start()
  try {
    const detail = await getAccountModelTestDetail(testId.value, { signal: detailRequest.signal, silent: true })
    if (detailRequest.isCurrent(requestId))
      result.value = detail
  }
  catch (cause) {
    detailRequest.fail(requestId, cause, background)
  }
  finally {
    detailRequest.finish(requestId)
  }
}

async function start() {
  if (!canSubmit.value)
    return
  submitting.value = true
  submitError.value = ''
  try {
    const response = await startAccountModelTest({
      accountId: props.account.id,
      kind: kind.value,
      prompt: prompt.value,
      modelId: selectedModelId.value,
    })
    testId.value = response.testId
    result.value = undefined
    emit('started')
    if (open.value)
      void loadResult()
  }
  catch (cause) {
    submitError.value = errorMessage(cause)
  }
  finally {
    submitting.value = false
  }
}

useIntervalFn(() => {
  if (open.value && running.value)
    void loadResult(true)
}, 2000)

watch(open, (value) => {
  if (!value) {
    models.invalidate()
    detailRequest.invalidate()
  }
})

if (props.seed) {
  kind.value = props.seed.kind
  prompt.value = props.seed.prompt
  if (props.seed.kind === 'image') {
    imageModelId.value = props.seed.modelId
  }
  else {
    modelId.value = props.seed.modelId
    modelOptions.value = [
      { value: modelId.value, label: modelId.value },
    ]
  }
}
void loadModels()
</script>

<template>
  <ZDialog v-model="open" title="降智测试" width="46rem">
    <div class="grid gap-4">
      <div class="rounded-z-card bg-z-fill-quaternary px-4 py-3">
        <AccountIdentityCell :account="account" show-plan />
      </div>
      <div class="grid gap-3 sm:grid-cols-2">
        <ZFormItem label="题目模板">
          <ZSelect :model-value="templateId" :options="visibleTemplates" aria-label="题目模板" :disabled="submitting" @update:model-value="applyTemplate" />
        </ZFormItem>
        <ZFormItem label="请求类型">
          <ZSelect :model-value="kind" :options="visibleKinds" aria-label="请求类型" :disabled="submitting" @update:model-value="value => kind = value || 'text'" />
        </ZFormItem>
      </div>
      <ZFormItem label="上游模型 ID">
        <div class="flex min-w-0 gap-2">
          <ZSelect v-if="kind !== 'image'" v-model="modelId" :options="modelOptions" class="min-w-0 flex-1" aria-label="上游模型 ID" placeholder="选择上游模型" filterable :loading="models.loading.value" :disabled="submitting" />
          <ZInput v-else v-model="imageModelId" class="min-w-0 flex-1" aria-label="上游图片模型 ID" placeholder="输入上游图片模型 ID" :disabled="submitting" />
          <ZIconButton v-if="kind !== 'image'" aria-label="刷新上游模型" :disabled="submitting || models.loading.value" @click="loadModels(true)">
            <RefreshCw class="size-4" />
          </ZIconButton>
        </div>
        <p v-if="models.error.value && kind !== 'image'" class="m-0 text-z-sm text-z-error-text">
          {{ models.error.value }}
        </p>
      </ZFormItem>
      <ZFormItem label="提示词">
        <ZTextarea v-model="prompt" aria-label="提示词" :rows="5" :disabled="submitting" placeholder="输入要发送给模型的内容" />
        <p v-if="promptTooLong" class="m-0 text-z-sm text-z-error-text">
          提示词不能超过 32 KiB
        </p>
        <ZButton v-if="templateId !== 'blank' && !submitting" size="small" class="justify-self-start" @click="applyTemplate()">
          恢复原题
        </ZButton>
      </ZFormItem>
      <p v-if="submitError || detailRequest.error.value" class="m-0 text-z-sm text-z-error-text" role="alert">
        {{ submitError || detailRequest.error.value }}
      </p>
      <AccountModelTestResult v-if="result" :result="result" />
      <p v-else-if="testId" class="m-0 text-z-sm text-z-text-secondary" role="status">
        已提交，关闭弹窗后测试会继续运行
      </p>
      <p v-else class="m-0 text-z-sm text-z-text-secondary">
        使用所选账号发送真实请求，消耗上游额度
      </p>
    </div>
    <template #footer>
      <ZButton @click="emit('history', account)">
        查看此账号历史
      </ZButton>
      <ZButton @click="open = false">
        关闭
      </ZButton>
      <ZButton type="primary" :loading="submitting" :disabled="!canSubmit" @click="start">
        {{ running ? '测试中' : result ? '再次测试' : '开始测试' }}
      </ZButton>
    </template>
  </ZDialog>
</template>
