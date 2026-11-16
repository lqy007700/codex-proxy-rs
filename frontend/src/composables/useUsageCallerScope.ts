import type { CallerUsageFilters } from '@/api/modules/usage'
import { computed } from 'vue'
import { useRoute } from 'vue-router'

export function useUsageCallerScope() {
  const route = useRoute()
  const callerFilters = computed(() => {
    const result: CallerUsageFilters = {}
    for (const name of [
      'callerMode',
      'callerSourceId',
      'callerUserId',
      'callerTokenId',
      'requestId',
      'clientApiKeyId',
      'accountId',
    ] as const) {
      if (typeof route.query[name] === 'string')
        result[name] = route.query[name]
    }
    return result
  })
  const scopedLabels = computed(() => [
    ['requestId', '请求'],
    ['callerSourceId', '来源'],
    ['callerUserId', '用户'],
    ['callerTokenId', 'Token'],
    ['clientApiKeyId', 'Key'],
    ['accountId', '账号'],
  ].flatMap(([name, label]) => typeof route.query[name!] === 'string'
    ? [`${label} ${route.query[name!]}`]
    : []))

  return { callerFilters, scopedLabels }
}
