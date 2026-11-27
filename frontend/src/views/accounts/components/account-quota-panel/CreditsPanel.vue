<script setup lang="ts">
import type { Account } from '@/api'
import { ZDialog, ZIconButton } from '@codex-proxy/ui'
import { Coins } from '@lucide/vue'
import { shallowRef } from 'vue'
import Credits from './Credits.vue'
import CreditsPolicy from './CreditsPolicy.vue'

defineProps<{ account: Account }>()
const emit = defineEmits<{ accountUpdated: [account: Account] }>()
const open = shallowRef(false)
</script>

<template>
  <ZIconButton size="small" aria-label="查看额度点数" aria-haspopup="dialog" :pressed="open" @click="open = true">
    <Coins class="size-3.5" />
  </ZIconButton>
  <ZDialog v-model="open" title="额度点数" width="36rem">
    <div class="grid gap-4">
      <section class="overflow-hidden rounded-z bg-z-fill-quaternary pt-4" aria-label="当前额度点数">
        <Credits :credits="account.quota.credits" />
      </section>
      <CreditsPolicy :account="account" @account-updated="emit('accountUpdated', $event)" />
    </div>
  </ZDialog>
</template>
