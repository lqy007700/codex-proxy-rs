-- 点数支出设置独立于上游额度事实，存量与新账号均默认关闭
alter table provider_accounts add column allow_credits boolean not null default false;

alter table provider_accounts drop constraint provider_accounts_quota_evidence_ck;
alter table provider_accounts add constraint provider_accounts_quota_evidence_ck check (
  quota_evidence is null or quota_evidence in (
    'provider_denied', 'account_limit_reached', 'usage_limit_reached',
    'payment_required', 'credits_available'
  )
);

-- 为已通过点数放行的快照补齐条件访问事实，并让 Provider 尽快复核窗口
with snapshots as (
  select id, coalesce(
    provider_quota_json #> '{rate_limits_by_limit_id,codex}',
    provider_quota_json -> 'rate_limit'
  ) as codex
  from provider_accounts
  where provider_kind = 'openai' and authentication_kind = 'oauth'
    and quota_access_state = 'allowed'
    and (provider_quota_json #> '{credits,has_credits}' = 'true'::jsonb
      or provider_quota_json #> '{credits,unlimited}' = 'true'::jsonb)
)
update provider_accounts a
set quota_access_state = 'exhausted', quota_evidence = 'credits_available',
    quota_reset_at = a.quota_access_observed_at
from snapshots s
where a.id = s.id and (
  s.codex -> 'allowed' = 'false'::jsonb
  or s.codex -> 'limit_reached' = 'true'::jsonb
  or jsonb_path_exists(s.codex, '$.* ? (@.used_percent >= 100 || @.limit_reached == true)')
);
