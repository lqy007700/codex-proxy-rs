-- 额度预警与失败率告警独立保存检查进度，共用邮件配置和收件人 outbox
alter table alert_settings add column quota_runtime_json jsonb not null default '{}'::jsonb
  check (jsonb_typeof(quota_runtime_json) = 'object');

create table quota_alert_windows (
  account_id text not null references provider_accounts(id) on delete cascade,
  window_key text not null,
  cycle_reset_at timestamptz not null,
  notified boolean not null default false,
  primary key (account_id, window_key)
);

alter table alert_notifications drop constraint alert_notifications_kind_check;
alter table alert_notifications add constraint alert_notifications_kind_check
  check (kind in ('triggered', 'reminder', 'recovered', 'test', 'quota_warning'));
