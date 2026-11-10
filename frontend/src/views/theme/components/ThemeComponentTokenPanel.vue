<script setup lang="ts">
import type { Theme, ThemeComponentOverrides, ThemeTokenName } from '@codex-proxy/ui/theme'
import type { Component } from 'vue'
import type {
  ThemeEditorComponent,
  ThemeEditorDraft,
} from '../composables/useThemeEditor'

import {
  AppWindow,
  Compass,
  CreditCard,
  FormInput,
  LayoutPanelLeft,
  MousePointerClick,
  TableProperties,
} from '@lucide/vue'
import { computed } from 'vue'

import ThemeColorTokenField from './ThemeColorTokenField.vue'
import ThemeNumberTokenField from './ThemeNumberTokenField.vue'
import ThemeTextTokenField from './ThemeTextTokenField.vue'

interface ComponentOption {
  id: ThemeEditorComponent
  label: string
  description: string
  icon: Component
}

interface EditableToken {
  name: ThemeTokenName
  label: string
  kind: 'color' | 'shadow' | 'number'
  description: string
  componentKey?: keyof ThemeComponentOverrides
  min?: number
  max?: number
}

const props = defineProps<{
  draft: ThemeEditorDraft
  resolved: Theme
  query: string
}>()

const emit = defineEmits<{
  change: [name: ThemeTokenName, value: string]
  reset: [name: ThemeTokenName]
  componentNumber: [key: keyof ThemeComponentOverrides, value: number]
  resetComponentNumber: [key: keyof ThemeComponentOverrides]
}>()

const component = defineModel<ThemeEditorComponent>({ required: true })

const components: readonly ComponentOption[] = [
  {
    id: 'action',
    label: 'Action',
    description: '按钮与动作',
    icon: MousePointerClick,
  },
  {
    id: 'form',
    label: 'Form',
    description: '表单与输入',
    icon: FormInput,
  },
  {
    id: 'surface',
    label: 'Surface',
    description: '卡片与容器',
    icon: CreditCard,
  },
  {
    id: 'data',
    label: 'Data Display',
    description: '数据展示',
    icon: TableProperties,
  },
  {
    id: 'navigation',
    label: 'Navigation',
    description: '导航',
    icon: Compass,
  },
  {
    id: 'layout',
    label: 'Layout',
    description: '布局与滚动',
    icon: LayoutPanelLeft,
  },
]

const tokenCatalog: Record<ThemeEditorComponent, readonly EditableToken[]> = {
  action: [
    {
      name: 'button-primary-color',
      label: '主按钮文字',
      kind: 'color',
      description: 'primaryColor',
    },
    {
      name: 'button-primary-bg',
      label: '主按钮背景',
      kind: 'color',
      description: 'primaryBg',
    },
    {
      name: 'button-primary-hover-bg',
      label: '主按钮 Hover',
      kind: 'color',
      description: 'primaryHoverBg',
    },
    {
      name: 'button-primary-active-bg',
      label: '主按钮 Active',
      kind: 'color',
      description: 'primaryActiveBg',
    },
    {
      name: 'button-secondary-bg',
      label: '次要按钮背景',
      kind: 'color',
      description: 'secondaryBg',
    },
    {
      name: 'button-secondary-hover-bg',
      label: '次要按钮 Hover',
      kind: 'color',
      description: 'secondaryHoverBg',
    },
    {
      name: 'button-secondary-active-bg',
      label: '次要按钮 Active',
      kind: 'color',
      description: 'secondaryActiveBg',
    },
    {
      name: 'icon-button-secondary-bg',
      label: '次要图标按钮背景',
      kind: 'color',
      description: 'secondaryBg',
    },
    {
      name: 'icon-button-secondary-hover-bg',
      label: '次要图标按钮 Hover',
      kind: 'color',
      description: 'secondaryHoverBg',
    },
    {
      name: 'icon-button-secondary-active-bg',
      label: '次要图标按钮 Active',
      kind: 'color',
      description: 'secondaryActiveBg',
    },
  ],
  form: [
    {
      name: 'input-bg',
      label: '输入框背景',
      kind: 'color',
      description: 'controlBg',
    },
    {
      name: 'input-hover-bg',
      label: '输入框 Hover',
      kind: 'color',
      description: 'hoverBg',
    },
    {
      name: 'input-active-bg',
      label: '输入框 Active',
      kind: 'color',
      description: 'activeBg',
    },
    {
      name: 'input-shadow',
      label: '输入框阴影',
      kind: 'shadow',
      description: 'shadow',
    },
    {
      name: 'input-hover-shadow',
      label: '输入框 Hover 阴影',
      kind: 'shadow',
      description: 'hoverShadow',
    },
    {
      name: 'input-active-shadow',
      label: '输入框焦点外圈',
      kind: 'shadow',
      description: 'activeShadow',
    },
  ],
  surface: [
    {
      name: 'card-bg',
      label: '卡片背景',
      kind: 'color',
      description: 'colorBgContainer',
    },
    {
      name: 'modal-bg',
      label: '弹窗背景',
      kind: 'color',
      description: 'contentBg',
    },
    {
      name: 'popover-header-bg',
      label: '浮层标题背景',
      kind: 'color',
      description: 'headerBg',
    },
    {
      name: 'card-border-radius',
      label: '卡片圆角',
      kind: 'number',
      description: 'borderRadius',
      componentKey: 'cardBorderRadius',
      min: 0,
      max: 32,
    },
    {
      name: 'card-shadow',
      label: '卡片阴影',
      kind: 'shadow',
      description: 'cardShadow',
    },
  ],
  data: [
    {
      name: 'table-row-height',
      label: '表格行高',
      kind: 'number',
      description: 'rowHeight',
      componentKey: 'tableRowHeight',
      min: 44,
      max: 84,
    },
    {
      name: 'table-header-bg',
      label: '表头背景',
      kind: 'color',
      description: 'headerBg',
    },
    {
      name: 'table-row-bg',
      label: '行背景',
      kind: 'color',
      description: 'rowBg',
    },
    {
      name: 'table-row-stripe-bg',
      label: '斑马纹背景',
      kind: 'color',
      description: 'rowStripeBg',
    },
    {
      name: 'table-row-hover-bg',
      label: '行 Hover',
      kind: 'color',
      description: 'rowHoverBg',
    },
    {
      name: 'table-row-selected-bg',
      label: '选中行背景',
      kind: 'color',
      description: 'rowSelectedBg',
    },
    {
      name: 'table-row-selected-hover-bg',
      label: '选中行 Hover',
      kind: 'color',
      description: 'rowSelectedHoverBg',
    },
    {
      name: 'progress-remaining-color',
      label: '进度轨道',
      kind: 'color',
      description: 'progressRemainingColor',
    },
  ],
  navigation: [
    {
      name: 'menu-item-selected-bg',
      label: '菜单选中背景',
      kind: 'color',
      description: 'itemSelectedBg',
    },
  ],
  layout: [
    {
      name: 'layout-sider-bg',
      label: '侧栏背景',
      kind: 'color',
      description: 'siderBg',
    },
    {
      name: 'layout-sider-shadow',
      label: '侧栏阴影',
      kind: 'shadow',
      description: 'siderShadow',
    },
    {
      name: 'scrollbar-thumb-bg',
      label: '滚动条滑块',
      kind: 'color',
      description: 'scrollbarThumbBg',
    },
    {
      name: 'scrollbar-thumb-hover-bg',
      label: '滚动条 Hover',
      kind: 'color',
      description: 'scrollbarThumbHoverBg',
    },
  ],
}

const activeComponent = computed(() =>
  components.find(option => option.id === component.value) ?? components[0]!,
)
const visibleTokens = computed(() => tokenCatalog[component.value].filter((token) => {
  const query = props.query.trim().toLocaleLowerCase()
  return !query || `${token.label} ${token.name} ${token.description}`.toLocaleLowerCase().includes(query)
}))

function overridden(name: ThemeTokenName) {
  return Object.hasOwn(props.draft.customization.tokenOverrides ?? {}, name)
}

function componentOverridden(key: keyof ThemeComponentOverrides | undefined) {
  return key
    ? Object.hasOwn(props.draft.customization.component ?? {}, key)
    : false
}

function numericTokenValue(name: ThemeTokenName) {
  return Number.parseFloat(props.resolved.tokens[name])
}
</script>

<template>
  <div class="grid gap-4">
    <section aria-labelledby="component-catalog-title">
      <div class="mb-2 flex items-center gap-2 px-1">
        <AppWindow class="size-3.5 text-z-primary-text" />
        <h3 id="component-catalog-title" class="m-0 text-z font-heavy text-z-text">
          Component Token
        </h3>
      </div>
      <div class="grid grid-cols-2 gap-2" role="listbox" aria-label="Component Token 组件族">
        <button
          v-for="option in components"
          :key="option.id"
          type="button"
          role="option"
          class="flex min-h-14 items-center gap-2.5 rounded-z-lg bg-z-fill-quaternary px-3 text-left outline-none transition-[background-color,box-shadow] duration-150 hover:bg-z-bg-text-hover focus-visible:ring-2 focus-visible:ring-z-control-outline"
          :class="component === option.id ? 'bg-z-control-item-bg-active shadow-z-tertiary' : undefined"
          :aria-selected="component === option.id"
          @click="component = option.id"
        >
          <component :is="option.icon" class="size-4 shrink-0" :class="component === option.id ? 'text-z-primary-text' : 'text-z-text-tertiary'" />
          <span class="min-w-0">
            <strong class="block truncate text-z-xs font-bold text-z-text">{{ option.label }}</strong>
            <span class="mt-0.5 block text-[9px] font-emphasis text-z-text-quaternary">{{ option.description }}</span>
          </span>
        </button>
      </div>
    </section>

    <section class="grid gap-2" :aria-label="`${activeComponent.label} Component Token`">
      <div class="rounded-z-lg bg-z-bg-container px-3 py-3 shadow-z-tertiary">
        <p class="m-0 text-[10px] leading-normal font-emphasis text-z-text-secondary">
          调整 {{ activeComponent.label }} 的 Component Token，未修改的值继续由全局 Seed / Alias 算法生成
        </p>
      </div>

      <template v-for="token in visibleTokens" :key="token.name">
        <ThemeColorTokenField
          v-if="token.kind === 'color'"
          :label="token.label"
          :token="token.description"
          :value="resolved.tokens[token.name]"
          :overridden="overridden(token.name)"
          @change="emit('change', token.name, $event)"
          @reset="emit('reset', token.name)"
        />
        <ThemeTextTokenField
          v-else-if="token.kind === 'shadow'"
          :label="token.label"
          :token="token.description"
          :value="resolved.tokens[token.name]"
          :overridden="overridden(token.name)"
          @change="emit('change', token.name, $event)"
          @reset="emit('reset', token.name)"
        />
        <ThemeNumberTokenField
          v-else-if="token.componentKey"
          :label="token.label"
          :token="token.description"
          :value="numericTokenValue(token.name)"
          :min="token.min ?? 0"
          :max="token.max ?? 100"
          :overridden="componentOverridden(token.componentKey)"
          @change="emit('componentNumber', token.componentKey, $event)"
          @reset="emit('resetComponentNumber', token.componentKey)"
        />
      </template>

      <div v-if="visibleTokens.length === 0" class="rounded-z-lg bg-z-fill-quaternary px-3 py-6 text-center text-z-xs font-emphasis text-z-text-quaternary">
        没有匹配的 Component Token
      </div>
    </section>
  </div>
</template>
