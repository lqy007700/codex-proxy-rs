alter table provider_accounts
  add column quota_limits jsonb not null default '{}'::jsonb,
  add column quota_paused boolean not null default false,
  add constraint provider_accounts_quota_limits_check check (
    jsonb_typeof(quota_limits) = 'object'
    and not jsonb_path_exists(
      quota_limits,
      '$.* ? (@.type() != "number" || @ < 1 || @ > 100 || @ != @.floor())'
    )
  );
