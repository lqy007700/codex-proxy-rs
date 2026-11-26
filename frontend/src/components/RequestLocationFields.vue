<script setup lang="ts">
import type { RequestLocation } from '@/api'
import { ZFormItem, ZInput } from '@codex-proxy/ui'
import CountryCodeField from '@/components/CountryCodeField.vue'

defineProps<{ disabled?: boolean }>()
const location = defineModel<RequestLocation>({ required: true })

function update(field: keyof RequestLocation, value: string) {
  location.value = { ...location.value, [field]: value }
}
</script>

<template>
  <div class="grid gap-4 sm:grid-cols-2">
    <CountryCodeField
      :model-value="location.country"
      :show-message="Boolean(location.country.trim())"
      :disabled="disabled"
      @update:model-value="update('country', $event)"
    />
    <ZFormItem label="地区" required>
      <ZInput :model-value="location.region" :disabled="disabled" aria-label="地区" placeholder="请输入地区" maxlength="128" @update:model-value="update('region', $event)" />
    </ZFormItem>
    <ZFormItem label="城市" required>
      <ZInput :model-value="location.city" :disabled="disabled" aria-label="城市" placeholder="请输入城市" maxlength="128" @update:model-value="update('city', $event)" />
    </ZFormItem>
    <ZFormItem label="IANA 时区" required>
      <ZInput :model-value="location.timezone" :disabled="disabled" aria-label="IANA 时区" placeholder="请输入 IANA 时区" @update:model-value="update('timezone', $event)" />
    </ZFormItem>
  </div>
</template>
