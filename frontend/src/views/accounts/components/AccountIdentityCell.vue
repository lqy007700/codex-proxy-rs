<script setup lang="ts">
import type { Account } from '@/api'

import { computed } from 'vue'

import AccountPlanBadge from '@/components/account/AccountPlanBadge.vue'
import { accountCompactIdentityLabel, accountIdentityLabel } from '@/utils/accountIdentity'
import { stablePresetVisualToneClass } from '@/utils/color'
import AccountFallbackIndicator from './AccountFallbackIndicator.vue'
import AccountNotesPopover from './AccountNotesPopover.vue'

type AccountIdentity = Pick<Account, 'id' | 'email' | 'planType' | 'planTypeDisplay'>
  & Partial<Pick<Account, 'accountId' | 'notes' | 'name' | 'authenticationKind' | 'schedulingRole'>>

const props = withDefaults(
  defineProps<{
    account: AccountIdentity
    size?: 'md' | 'lg'
    showPlan?: boolean
    showNotes?: boolean
    nameOnly?: boolean
    titleMode?: 'local-part' | 'email'
    metaPosition?: 'title' | 'secondary'
    metaSize?: 'xs' | 'sm'
  }>(),
  {
    size: 'md',
    showPlan: false,
    showNotes: false,
    nameOnly: false,
    titleMode: 'local-part',
    metaPosition: 'title',
    metaSize: 'sm',
  },
)

const emailText = computed(() => accountIdentityLabel(props.account))

const visibleNotes = computed(() => props.showNotes ? props.account.notes : undefined)

const displayTitle = computed(() => {
  if (props.nameOnly)
    return accountCompactIdentityLabel(props.account)
  if (visibleNotes.value || props.titleMode === 'email' || props.account.authenticationKind === 'api_key')
    return emailText.value
  return emailText.value.split('@')[0]
})

const secondaryText = computed(() =>
  props.nameOnly || props.titleMode === 'email' || props.account.authenticationKind === 'api_key'
    ? null
    : emailText.value,
)

const initial = computed(() => displayTitle.value.slice(0, 1).toUpperCase())

const avatarSizeClass = computed(() =>
  props.size === 'lg'
    ? 'size-10 text-z-xl'
    : 'size-9 text-z',
)

const secondaryClass = computed(() =>
  props.size === 'lg'
    ? 'mt-1 text-z-sm text-z-text-secondary'
    : 'mt-0.5 font-mono text-z-xs text-z-text-quaternary',
)

const metaGapClass = computed(() => props.metaSize === 'xs'
  ? 'gap-1'
  : 'gap-1.5')

const avatarToneClass = computed(() => {
  const identity = props.account.id || props.account.email || displayTitle.value
  return stablePresetVisualToneClass(identity)
})
</script>

<template>
  <div class="flex min-w-0 items-center gap-3">
    <span
      class="inline-flex shrink-0 items-center justify-center rounded-lg font-extrabold"
      :class="[avatarSizeClass, avatarToneClass]"
    >
      {{ initial }}
    </span>
    <div class="min-w-0 flex-1">
      <div class="flex min-w-0 items-center gap-2">
        <span class="min-w-0 truncate text-z font-heavy text-z-text" :class="{ 'flex-1': !nameOnly }" :title="displayTitle">
          {{ displayTitle }}
        </span>
        <AccountFallbackIndicator v-if="account.schedulingRole === 'fallback'" />
        <span
          v-if="metaPosition === 'title' && (showPlan || $slots.meta)"
          class="inline-flex shrink-0 items-center justify-end"
          :class="metaGapClass"
        >
          <slot name="meta" />
          <AccountPlanBadge v-if="showPlan" :authentication-kind="account.authenticationKind" :plan-type="account.planType" :plan-type-display="account.planTypeDisplay" :size="metaSize" />
        </span>
      </div>
      <div
        v-if="metaPosition === 'secondary' && (showPlan || $slots.meta)"
        class="mt-0.5 inline-flex min-w-0 items-center"
        :class="metaGapClass"
      >
        <slot name="meta" />
        <AccountPlanBadge v-if="showPlan" :authentication-kind="account.authenticationKind" :plan-type="account.planType" :plan-type-display="account.planTypeDisplay" :size="metaSize" />
      </div>
      <AccountNotesPopover v-else-if="visibleNotes" :notes="visibleNotes" class="mt-0.5" />
      <div v-else-if="secondaryText" class="truncate font-emphasis" :class="secondaryClass">
        {{ secondaryText }}
      </div>
    </div>
  </div>
</template>
