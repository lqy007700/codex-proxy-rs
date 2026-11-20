-- 账号 Fast 策略不继承分组，保留旧列和值供版本回退
alter table provider_accounts
  add column fast_mode text not null default 'default'
    check (fast_mode in ('default', 'enabled', 'disabled'));
