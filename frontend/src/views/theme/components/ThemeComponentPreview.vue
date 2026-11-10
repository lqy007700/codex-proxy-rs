<script setup lang="ts">
import type { PaginationState } from '@codex-proxy/ui'

import { defineTableColumns, ZFormItem as FormItem, ZButton, ZCard, ZCheckbox, ZColorPicker, ZEmpty, ZForm, ZIconButton, ZInput, ZInputNumber, ZMenuItem, ZMotionIcon, ZPagination, ZPopover, ZRadio, ZSegmented, ZSelect, ZSkeleton, ZSlider, ZSwitch, ZTable, ZTag, ZTextarea } from '@codex-proxy/ui'

import {
  Bell,
  Check,
  CircleAlert,
  Download,
  Info,
  KeyRound,
  MoreHorizontal,
  Plus,
  Search,
  Trash2,
  TriangleAlert,
} from '@lucide/vue'
import { shallowRef } from 'vue'

interface PreviewTableRow {
  id: string
  account: string
  provider: string
  status: '正常' | '限流' | '停用' | '异常'
  requests: string
  latency: string
  updatedAt: string
}

const search = shallowRef('中继账号')
const notes = shallowRef('优先保持当前账号，额度耗尽后再切换')
const brandColor = shallowRef('#5983F4')
const invalidKey = shallowRef('sk-invalid')
const disabledText = shallowRef('不可编辑')
const provider = shallowRef('openai')
const view = shallowRef('all')
const notifications = shallowRef(true)
const autoRotate = shallowRef(false)
const selected = shallowRef(true)
const partialSelected = shallowRef(false)
const strategy = shallowRef('balanced')
const controlHeight = shallowRef(38)
const syncTagVisible = shallowRef(true)

const providerOptions = [
  { label: 'OpenAI', value: 'openai' },
  { label: 'xAI', value: 'xai' },
  { label: '全部平台', value: 'all' },
]
const viewOptions = [
  { label: '全部', value: 'all' },
  { label: '正常', value: 'active' },
  { label: '受限', value: 'limited' },
]
const themeSwatches = [
  { label: 'Container', className: 'bg-z-bg-container' },
  { label: 'Fill Alter', className: 'bg-z-fill-alter' },
  { label: 'Elevated', className: 'bg-z-bg-elevated' },
  { label: 'Accent', className: 'bg-z-primary' },
  { label: 'Success', className: 'bg-z-success' },
  { label: 'Warning', className: 'bg-z-warning' },
  { label: 'Error', className: 'bg-z-error' },
]
const progressItems = [
  { label: '账号可用率', value: '94%', width: '94%' },
  { label: '今日额度', value: '68%', width: '68%' },
  { label: '批量导入', value: '12 / 20', width: '60%' },
]
const tableColumns = defineTableColumns<PreviewTableRow>([
  {
    key: 'account',
    label: '账号',
    kind: 'identity',
    size: '3xl',
  },
  {
    key: 'provider',
    label: '平台',
    kind: 'status',
    size: 'md',
  },
  {
    key: 'status',
    label: '状态',
    kind: 'status',
    size: 'md',
  },
  {
    key: 'requests',
    label: '今日请求',
    kind: 'numeric',
    size: 'lg',
  },
  {
    key: 'latency',
    label: '首字延迟',
    kind: 'numeric',
    size: 'lg',
  },
  {
    key: 'updatedAt',
    label: '最近使用',
    kind: 'datetime',
    size: 'lg',
  },
])
const tableRows: PreviewTableRow[] = [
  {
    id: 'relay',
    account: 'relay@example.com',
    provider: 'OpenAI',
    status: '正常',
    requests: '12,840',
    latency: '228 ms',
    updatedAt: '刚刚',
  },
  {
    id: 'gateway',
    account: 'gateway@example.com',
    provider: 'xAI',
    status: '正常',
    requests: '9,462',
    latency: '316 ms',
    updatedAt: '2 分钟前',
  },
  {
    id: 'quota',
    account: 'quota@example.com',
    provider: 'OpenAI',
    status: '限流',
    requests: '7,318',
    latency: '486 ms',
    updatedAt: '5 分钟前',
  },
  {
    id: 'batch',
    account: 'batch@example.com',
    provider: 'OpenAI',
    status: '正常',
    requests: '5,904',
    latency: '264 ms',
    updatedAt: '8 分钟前',
  },
  {
    id: 'fallback',
    account: 'fallback@example.com',
    provider: 'xAI',
    status: '异常',
    requests: '3,172',
    latency: '—',
    updatedAt: '12 分钟前',
  },
  {
    id: 'disabled',
    account: 'disabled@example.com',
    provider: 'OpenAI',
    status: '停用',
    requests: '0',
    latency: '—',
    updatedAt: '2 小时前',
  },
]
const pagination: PaginationState = {
  currentPage: 2,
  pageSize: 20,
  total: 1_182,
}

function statusClass(status: PreviewTableRow['status']) {
  if (status === '正常')
    return 'bg-z-success-container text-z-success-on-container'
  if (status === '限流')
    return 'bg-z-warning-container text-z-warning-on-container'
  if (status === '异常')
    return 'bg-z-error-container text-z-error-on-container'
  return 'bg-z-fill-tertiary text-z-text-secondary'
}
</script>

<template>
  <div class="grid h-full min-w-0 grid-rows-[80px_352px_290px_520px_374px] gap-5 overflow-hidden rounded-z-card bg-(--z-card-bg) p-6 shadow-z-card">
    <header class="flex items-center justify-between gap-8 px-1">
      <div class="min-w-0">
        <p class="m-0 font-mono text-z-xs font-heavy tracking-[0.16em] text-z-primary-text uppercase">
          Component specimen
        </p>
        <h1 class="mt-2 mb-0 text-[30px] leading-none font-extrabold text-z-text">
          组件概览
        </h1>
        <p class="mt-2 mb-0 text-z font-emphasis text-z-text-secondary">
          在同一张校样板中检查控制态、数据密度与浮层关系
        </p>
      </div>

      <div class="flex shrink-0 items-center gap-6">
        <span class="rounded-full bg-z-primary-container px-3 py-1.5 font-mono text-[10px] font-heavy tracking-wide text-z-primary-on-container uppercase">
          21 Base Components
        </span>
        <div class="flex items-center gap-5" aria-label="当前主题语义色">
          <div
            v-for="item in themeSwatches"
            :key="item.label"
            class="grid justify-items-center gap-1.5"
          >
            <span class="size-6 rounded-z shadow-z-tertiary" :class="item.className" />
            <span class="font-mono text-[10px] font-heavy text-z-text-quaternary">{{ item.label }}</span>
          </div>
        </div>
      </div>
    </header>

    <section class="grid min-w-0 grid-cols-12 gap-6" aria-label="基础控制组件">
      <ZCard padding="compact" class="col-span-7 h-full min-w-0 bg-z-fill-quaternary! shadow-none!" title="按钮与动作" description="ZButton · ZIconButton · ZMotionIcon">
        <template #body>
          <div class="grid flex-1 content-start gap-6">
            <div>
              <p class="mt-0 mb-3 font-mono text-z-xs font-heavy text-z-text-quaternary">
                Button / action
              </p>
              <div class="flex flex-wrap items-center gap-3">
                <ZButton type="primary">
                  <template #icon>
                    <Plus class="size-4" />
                  </template>
                  创建账号
                </ZButton>
                <ZButton>
                  <template #icon>
                    <Download class="size-4" />
                  </template>
                  导出数据
                </ZButton>
                <ZButton variant="text">
                  稍后处理
                </ZButton>
                <ZButton type="danger" variant="plain">
                  <template #icon>
                    <Trash2 class="size-4" />
                  </template>
                  删除
                </ZButton>
              </div>
            </div>

            <div class="grid grid-cols-[1fr_auto] items-end gap-5 rounded-z-lg bg-z-fill-quaternary p-4">
              <div>
                <p class="mt-0 mb-3 text-z-sm font-heavy text-z-text-secondary">
                  尺寸与禁用状态
                </p>
                <div class="flex flex-wrap items-center gap-3">
                  <ZButton size="small" type="primary">
                    Small
                  </ZButton>
                  <ZButton size="default" type="primary">
                    Medium
                  </ZButton>
                  <ZButton size="large" type="primary">
                    Large
                  </ZButton>
                  <ZButton disabled>
                    不可操作
                  </ZButton>
                  <ZButton loading>
                    处理中
                  </ZButton>
                </div>
              </div>
              <div class="flex items-center gap-2">
                <ZIconButton aria-label="通知" variant="solid">
                  <ZMotionIcon>
                    <Bell />
                  </ZMotionIcon>
                </ZIconButton>
                <ZIconButton aria-label="更多操作" variant="solid">
                  <MoreHorizontal />
                </ZIconButton>
                <ZIconButton aria-label="删除记录" type="danger">
                  <Trash2 />
                </ZIconButton>
              </div>
            </div>
          </div>
        </template>
      </ZCard>

      <ZCard padding="compact" class="col-span-5 h-full min-w-0 bg-z-fill-quaternary! shadow-none!" title="表单与输入" description="ZInput · ZSelect · ZInputNumber · ZSlider · ZTextarea · ZColorPicker">
        <template #body>
          <ZForm class="grid-cols-2 gap-3!">
            <ZInput v-model="search" aria-label="搜索账号" placeholder="搜索账号">
              <template #prefix>
                <Search class="size-4" />
              </template>
            </ZInput>
            <ZSelect v-model="provider" aria-label="选择平台" :options="providerOptions" />
            <ZInput v-model="invalidKey" aria-label="访问密钥错误示例" aria-invalid="true">
              <template #prefix>
                <KeyRound class="size-4" />
              </template>
            </ZInput>
            <ZInput v-model="disabledText" aria-label="禁用输入框" disabled />
            <FormItem label="品牌颜色" description="ZColorPicker">
              <ZColorPicker
                v-model="brandColor"
                aria-label="编辑品牌颜色"
                :allow-alpha="false"
                :presets="['#5983F4', '#0F766E', '#7C3AED', '#475569']"
              />
            </FormItem>
            <FormItem label="调度备注" description="ZTextarea">
              <ZTextarea v-model="notes" aria-label="调度备注" :rows="2" />
            </FormItem>
            <FormItem class="col-span-2" label="控件高度" description="ZInputNumber · ZSlider">
              <div class="flex items-center gap-3">
                <ZInputNumber
                  v-model="controlHeight"
                  size="small"
                  aria-label="组件示例控件高度"
                  :min="28"
                  :max="52"
                  unit="px"
                />
                <ZSlider
                  v-model="controlHeight"
                  class="min-w-0 flex-1"
                  aria-label="组件示例控件高度"
                  :min="28"
                  :max="52"
                  unit="px"
                />
              </div>
            </FormItem>
          </ZForm>
        </template>
      </ZCard>
    </section>

    <section class="grid min-w-0 grid-cols-12 gap-6" aria-label="选择与反馈组件">
      <ZCard padding="compact" class="col-span-4 h-full min-w-0 bg-z-fill-quaternary! shadow-none!" title="选择与切换" description="ZSegmented · ZSwitch · ZCheckbox · ZRadio">
        <template #body>
          <div class="grid gap-5">
            <ZSegmented v-model="view" aria-label="账号视图" :options="viewOptions" class="w-full" />
            <div class="flex flex-wrap items-center gap-x-7 gap-y-4">
              <ZSwitch v-model="notifications" label="实时通知" />
              <ZSwitch v-model="autoRotate" label="自动轮换" />
              <ZCheckbox v-model="selected" label="选择当前页" />
              <ZCheckbox v-model="partialSelected" label="部分选择" indeterminate />
            </div>
            <div class="flex items-center gap-6">
              <ZRadio v-model="strategy" value="balanced" name="strategy" label="均衡" />
              <ZRadio v-model="strategy" value="sticky" name="strategy" label="粘性" />
              <ZRadio v-model="strategy" value="manual" name="strategy" label="手动" />
            </div>
          </div>
        </template>
      </ZCard>

      <ZCard padding="compact" class="col-span-4 h-full min-w-0 bg-z-fill-quaternary! shadow-none!" title="反馈语义" description="Success · Warning · Error · Info">
        <template #body>
          <div class="grid gap-2.5">
            <div class="flex items-center gap-3 rounded-z bg-z-success-container px-3.5 py-3 text-z-success-on-container">
              <Check class="size-4 shrink-0" /><strong class="text-z-sm">主题设置已保存</strong>
            </div>
            <div class="flex items-center gap-3 rounded-z bg-z-warning-container px-3.5 py-3 text-z-warning-on-container">
              <TriangleAlert class="size-4 shrink-0" /><strong class="text-z-sm">3 个账号额度接近上限</strong>
            </div>
            <div class="flex items-center gap-3 rounded-z bg-z-error-container px-3.5 py-3 text-z-error-on-container">
              <CircleAlert class="size-4 shrink-0" /><strong class="text-z-sm">连接测试未通过</strong>
            </div>
            <div class="flex items-center gap-3 rounded-z bg-z-info-container px-3.5 py-3 text-z-info-on-container">
              <Info class="size-4 shrink-0" /><strong class="text-z-sm">调度规则将在下次请求生效</strong>
            </div>
          </div>
        </template>
      </ZCard>

      <ZCard padding="compact" class="col-span-4 h-full min-w-0 bg-z-fill-quaternary! shadow-none!" title="状态与进度" description="ZTag · Progress Token">
        <template #body>
          <div class="grid gap-4">
            <div class="flex flex-wrap gap-2">
              <ZTag type="success" round>
                运行正常
              </ZTag>
              <ZTag type="warning" round>
                额度受限
              </ZTag>
              <ZTag type="danger" round>
                需要处理
              </ZTag>
              <ZTag v-if="syncTagVisible" type="info" round closable @close="syncTagVisible = false">
                同步中
              </ZTag>
              <ZButton v-else size="small" variant="text" @click="syncTagVisible = true">
                恢复标签
              </ZButton>
            </div>
            <div v-for="item in progressItems" :key="item.label" class="grid gap-2">
              <div class="flex justify-between text-z-sm font-heavy text-z-text-secondary">
                <span>{{ item.label }}</span><span class="font-mono text-z-text">{{ item.value }}</span>
              </div>
              <div class="h-2 overflow-hidden rounded-full bg-z-progress-remaining">
                <div class="h-full rounded-full bg-z-primary" :style="{ width: item.width }" />
              </div>
            </div>
          </div>
        </template>
      </ZCard>
    </section>

    <ZCard padding="compact" class="h-full min-w-0 bg-z-fill-quaternary! shadow-none!" title="数据表格" description="ZTable · ZScrollbar · ZPagination（标准行高）">
      <template #actions>
        <ZInput aria-label="筛选表格" size="small" placeholder="筛选账号" class="w-64">
          <template #prefix>
            <Search class="size-3.5" />
          </template>
        </ZInput>
      </template>
      <template #body>
        <div class="grid min-h-0 flex-1 grid-rows-[minmax(0,1fr)_auto]">
          <ZTable :columns="tableColumns" :data="tableRows">
            <template #account="{ row }">
              <div class="grid gap-1">
                <strong class="font-mono text-z-sm text-z-text">{{ row.account }}</strong>
                <span class="text-[10px] font-emphasis text-z-text-quaternary">credential_{{ row.id }}</span>
              </div>
            </template>
            <template #status="{ row }">
              <span class="inline-flex rounded-full px-2.5 py-1 text-z-xs font-heavy" :class="statusClass(row.status)">
                {{ row.status }}
              </span>
            </template>
          </ZTable>
          <ZPagination v-bind="pagination" :disabled="false" />
        </div>
      </template>
    </ZCard>

    <section class="grid min-w-0 grid-cols-12 gap-6 overflow-hidden" aria-label="边界状态组件">
      <ZCard padding="compact" class="col-span-4 h-full min-w-0 bg-z-fill-quaternary! shadow-none!" title="空状态" description="ZEmpty · ZButton">
        <template #body>
          <ZEmpty title="暂无匹配账号" description="调整筛选条件，或添加一个新的上游账号" class="flex-1">
            <template #action>
              <ZButton size="small" type="primary">
                添加账号
              </ZButton>
            </template>
          </ZEmpty>
        </template>
      </ZCard>

      <ZCard padding="compact" class="col-span-4 h-full min-w-0 bg-z-fill-quaternary! shadow-none!" title="加载与骨架" description="ZSkeleton · ZButton loading">
        <template #body>
          <div class="grid flex-1 content-between gap-5 rounded-z-lg bg-z-fill-quaternary p-4">
            <div class="grid gap-4">
              <div class="flex items-center gap-3">
                <ZSkeleton class="size-10 shrink-0" />
                <div class="grid flex-1 gap-2">
                  <ZSkeleton shape="text" class="w-2/5" />
                  <ZSkeleton shape="text" class="h-2.5 w-3/5 opacity-60" />
                </div>
              </div>
              <ZSkeleton class="h-18 opacity-60" />
              <div class="grid grid-cols-3 gap-3">
                <ZSkeleton v-for="index in 3" :key="index" class="h-12 opacity-60" />
              </div>
            </div>
            <div class="flex items-center justify-between">
              <span class="text-z-sm font-emphasis text-z-text-secondary">正在同步账号状态</span>
              <ZButton size="small" loading>
                同步中
              </ZButton>
            </div>
          </div>
        </template>
      </ZCard>

      <ZCard padding="compact" class="col-span-4 h-full min-w-0 bg-z-fill-quaternary! shadow-none!" title="浮层与菜单" description="ZPopover · ZMenuItem · Elevated Surface">
        <template #body>
          <div class="grid min-w-0 flex-1 grid-rows-[auto_1fr_auto] overflow-hidden rounded-z-lg bg-z-bg-container p-4">
            <div class="flex items-center justify-between rounded-z bg-z-bg-container px-4 py-3 shadow-z-tertiary">
              <div>
                <strong class="block text-z-sm text-z-text">relay@example.com</strong>
                <span class="mt-1 block text-[10px] text-z-text-quaternary">OpenAI · OAuth</span>
              </div>
              <ZPopover>
                <template #reference>
                  <ZIconButton aria-label="账号操作" variant="solid">
                    <MoreHorizontal />
                  </ZIconButton>
                </template>
                <div class="w-64 max-w-full p-2">
                  <ZMenuItem>编辑账号</ZMenuItem>
                  <ZMenuItem>测试连接</ZMenuItem>
                  <ZMenuItem type="danger">
                    停用账号
                  </ZMenuItem>
                </div>
              </ZPopover>
            </div>

            <div class="mt-3 ml-auto w-64 max-w-full self-start rounded-z-lg bg-z-bg-elevated p-2 shadow-cp">
              <ZMenuItem>
                <template #icon>
                  <KeyRound class="size-3.5" />
                </template>
                编辑账号
              </ZMenuItem>
              <ZMenuItem>
                <template #icon>
                  <Info class="size-3.5" />
                </template>
                测试连接
              </ZMenuItem>
              <ZMenuItem type="danger">
                <template #icon>
                  <Trash2 class="size-3.5" />
                </template>
                停用账号
              </ZMenuItem>
            </div>

            <div class="flex min-w-0 items-center gap-3 rounded-z bg-z-bg-spotlight px-4 py-3 text-z-text-light-solid shadow-z-popup">
              <Info class="size-4 shrink-0" />
              <span class="min-w-0 text-z-xs font-heavy">浮层始终使用独立 Surface 与 Shadow Token</span>
            </div>
          </div>
        </template>
      </ZCard>
    </section>
  </div>
</template>
