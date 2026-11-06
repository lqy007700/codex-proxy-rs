import type { Account } from '@/api'

type AccountIdentity = Pick<Account, 'id' | 'email'>
  & Partial<Pick<Account, 'accountId' | 'name' | 'authenticationKind'>>

export function accountIdentityLabel(account: AccountIdentity): string {
  if (account.authenticationKind === 'api_key' && account.name)
    return account.name
  const email = account.email?.trim()
  if (email)
    return email
  if (typeof account.accountId === 'string')
    return account.accountId
  return String(account.id)
}

export function accountCompactIdentityLabel(account: AccountIdentity): string {
  const label = accountIdentityLabel(account)
  if (account.authenticationKind === 'api_key' || !account.email?.trim())
    return label
  return label.split('@')[0]
}
