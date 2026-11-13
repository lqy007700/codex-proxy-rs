import type { Account } from '@/api'

export function affinityTargetDisabled(account: Account) {
  return !account.enabled || account.schedulingRole === 'fallback'
    || ['credential_invalid', 'account_banned'].includes(account.errorReason ?? '')
}
