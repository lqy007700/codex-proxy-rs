<script setup lang="ts">
import type { SidebarItem } from '@codex-proxy/ui'
import { ZIconButton, ZMotionIcon, ZSidebar } from '@codex-proxy/ui'
import {
  ArrowUpCircle,
  ChartNoAxesColumn,
  FolderTree,
  Info,
  KeyRound,
  LayoutDashboard,
  LogOut,
  Moon,
  Network,
  Palette,
  PanelLeftClose,
  PanelLeftOpen,
  Settings,
  Sun,
  Users,
} from '@lucide/vue'
import { storeToRefs } from 'pinia'
import { computed, useTemplateRef } from 'vue'
import { isNavigationFailure, NavigationFailureType, useRoute, useRouter } from 'vue-router'
import AppBrandMark from '@/components/AppBrandMark.vue'
import { useAuthStore } from '@/stores/modules/auth'
import { useSystemUpdateStore } from '@/stores/modules/system-update'
import { useThemeStore } from '@/stores/modules/theme'

const props = withDefaults(defineProps<{ collapsed?: boolean, mobile?: boolean }>(), { collapsed: false, mobile: false })
const emit = defineEmits<{ close: [], navigate: [], openAbout: [], openSystemUpdate: [], toggle: [] }>()
const sidebar = useTemplateRef<InstanceType<typeof ZSidebar>>('sidebar')
const route = useRoute()
const router = useRouter()
const authStore = useAuthStore()
const { version, hasUpdate } = storeToRefs(useSystemUpdateStore())
const themeStore = useThemeStore()
const { effectiveTheme } = storeToRefs(themeStore)
const { toggleTheme } = themeStore
const isCollapsed = computed(() => !props.mobile && props.collapsed)
const navItems: SidebarItem[] = [
  { label: '概览', icon: LayoutDashboard, index: '/' },
  { label: '账号管理', icon: Users, index: '/accounts' },
  { label: '代理管理', icon: Network, index: '/proxies' },
  { label: '分组管理', icon: FolderTree, index: '/groups' },
  { label: 'API 密钥', icon: KeyRound, index: '/keys' },
  { label: '账号亲和', icon: Network, index: '/affinity' },
  { label: '使用统计', icon: ChartNoAxesColumn, index: '/usage' },
  { label: '外观', icon: Palette, index: '/theme' },
  { label: '系统设置', icon: Settings, index: '/settings' },
]

const activeIndex = computed(() => navItems.find(item => item.index !== '/' && route.path.startsWith(item.index))?.index ?? '/')
const themeToggleLabel = computed(() => effectiveTheme.value === 'dark'
  ? '切换浅色模式'
  : '切换暗黑模式')
const versionText = computed(() => version.value?.version.trim() ?? '')
const hasVersionLabel = computed(() => versionText.value.length > 0)
const versionLabel = computed(() => `v${versionText.value}`)
const updateButtonLabel = computed(() => hasUpdate.value ? '发现新版本，点击更新' : '检查更新')

async function handleSelect(index: string) {
  // 路由守卫可能取消跳转，选中状态以实际路由为准
  sidebar.value?.updateActiveIndex(activeIndex.value)
  const failure = await router.push(index)
  if (!failure || isNavigationFailure(failure, NavigationFailureType.duplicated))
    emit('navigate')
}

function openSystemUpdate() {
  emit('openSystemUpdate')
}

async function handleLogout() {
  if (!await authStore.logout())
    return
  await router.push('/login')
  emit('navigate')
}
</script>

<template>
  <ZSidebar
    ref="sidebar"
    class="z-20"
    :class="mobile ? 'flex' : 'hidden min-[961px]:flex'"
    :items="navItems"
    :default-active="activeIndex"
    :collapse="isCollapsed"
    navigation-label="主导航"
    @select="handleSelect"
  >
    <template #brand-icon>
      <ZMotionIcon variant="brand" class="relative -top-0.5 inline-flex size-11 items-center justify-center rounded-z">
        <AppBrandMark class="block size-11 select-none" />
      </ZMotionIcon>
    </template>
    <template #brand>
      <strong class="text-base leading-[1.1] font-heavy text-z-text"> Codex Proxy </strong>
      <span class="mt-1.5 flex h-4.5 min-w-0 items-center gap-2">
        <span class="shrink-0 text-xs leading-none font-emphasis text-z-text-secondary"> Rust build </span>
        <button
          v-if="hasVersionLabel"
          type="button"
          class="inline-flex h-4.5 min-w-0 cursor-pointer items-center gap-1 rounded-z-sm border-0 px-1.5 font-mono text-[10px] leading-none font-bold transition-colors outline-none focus-visible:ring-2 focus-visible:ring-z-control-outline focus-visible:ring-offset-2 focus-visible:ring-offset-z-bg-container"
          :class="[
            hasUpdate
              ? 'bg-z-success-container text-z-success-on-container hover:bg-z-success-container-hover'
              : 'bg-z-fill-quaternary text-z-text-quaternary hover:bg-z-fill-tertiary hover:text-z-text-secondary',
          ]"
          :title="updateButtonLabel"
          @click="openSystemUpdate"
        >
          <span>{{ versionLabel }}</span>
          <ArrowUpCircle v-if="hasUpdate" class="size-3 shrink-0 text-z-success" />
        </button>
      </span>
    </template>
    <template #footer>
      <div
        class="bg-z-fill-quaternary"
        :class="isCollapsed ? 'grid gap-1 rounded-z p-1' : 'flex h-11 items-center justify-between rounded-z-lg px-2'"
      >
        <span
          v-if="!isCollapsed"
          class="inline-flex whitespace-nowrap h-7 items-center gap-1.5 rounded-lg bg-z-success-container px-2.5 text-xs leading-none font-emphasis text-z-success-on-container"
        >
          <i class="size-1.5 rounded-full bg-z-success" />
          在线
        </span>

        <div class="flex items-center" :class="isCollapsed ? 'grid gap-1' : 'gap-1'">
          <ZIconButton
            v-if="isCollapsed && hasUpdate"
            type="success" variant="plain"
            size="default"
            :aria-label="updateButtonLabel"
            @click="openSystemUpdate"
          >
            <ArrowUpCircle :size="19" />
          </ZIconButton>

          <ZIconButton
            :size="isCollapsed ? 'default' : 'small'"
            aria-label="退出登录"
            type="danger"
            :disabled="authStore.loading || !authStore.isAuthenticated"
            @click="handleLogout"
          >
            <LogOut :size="isCollapsed ? 19 : 18" />
          </ZIconButton>

          <ZIconButton
            :size="isCollapsed ? 'default' : 'small'"
            :aria-label="themeToggleLabel"
            @click="toggleTheme($event)"
          >
            <Sun v-if="effectiveTheme === 'dark'" :size="isCollapsed ? 19 : 18" />
            <Moon v-else :size="isCollapsed ? 19 : 18" />
          </ZIconButton>

          <ZIconButton :size="isCollapsed ? 'default' : 'small'" aria-label="关于" @click="emit('openAbout')">
            <Info :size="isCollapsed ? 19 : 18" />
          </ZIconButton>

          <ZIconButton v-if="mobile" size="small" aria-label="关闭侧边栏" @click="emit('close')">
            <PanelLeftClose :size="18" />
          </ZIconButton>

          <ZIconButton
            v-else
            :size="isCollapsed ? 'default' : 'small'"
            data-sidebar-toggle
            :aria-label="isCollapsed ? '展开侧边栏' : '收缩侧边栏'"
            @click="emit('toggle')"
          >
            <PanelLeftOpen v-if="isCollapsed" :size="19" />
            <PanelLeftClose v-else :size="18" />
          </ZIconButton>
        </div>
      </div>
    </template>
  </ZSidebar>
</template>
