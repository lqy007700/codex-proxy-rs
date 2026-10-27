-- 邮件告警独立持有配置与评估状态，不参与请求运行设置的版本发布
create table alert_settings (
  id smallint primary key check (id = 1),
  revision bigint not null default 1 check (revision > 0),
  values_json jsonb not null check (jsonb_typeof(values_json) = 'object'),
  smtp_password text,
  runtime_json jsonb not null check (jsonb_typeof(runtime_json) = 'object'),
  last_test_at timestamptz,
  updated_at timestamptz not null default now()
);

insert into alert_settings (id, values_json, runtime_json) values (
  1,
  '{"enabled":false,"windowMinutes":5,"minimumRequests":20,"failureThresholdPercent":20,"consecutiveEvaluations":2,"recoveryThresholdPercent":10,"reminderMinutes":30,"recoveryNotification":true,"smtpHost":"","smtpPort":465,"smtpSecurity":"tls","smtpUsername":"","senderAddress":"","senderName":"Codex Proxy","recipients":[]}',
  '{"generation":0,"phase":"normal","evaluationStatus":"disabled","abnormalCount":0,"recoveryCount":0,"checkedAt":null,"sample":null,"activeEventId":null,"openedAt":null,"lastNotificationAt":null}'
);

create table alert_events (
  id text primary key,
  status text not null check (status in ('firing', 'recovered', 'cancelled')),
  opened_at timestamptz not null,
  closed_at timestamptz,
  sample_json jsonb not null check (jsonb_typeof(sample_json) = 'object'),
  check ((status = 'firing') = (closed_at is null))
);
create index alert_events_recent on alert_events (opened_at desc, id desc);

-- 每个收件人独立投递，部分成功不能导致已成功的收件人重复收到重试邮件
create table alert_notifications (
  id text primary key,
  event_id text references alert_events (id) on delete cascade,
  kind text not null check (kind in ('triggered', 'reminder', 'recovered', 'test')),
  settings_revision bigint not null check (settings_revision > 0),
  notification_key text not null,
  recipient text not null,
  subject text not null,
  body text not null,
  status text not null default 'pending'
    check (status in ('pending', 'sending', 'sent', 'failed', 'cancelled')),
  attempts integer not null default 0 check (attempts between 0 and 3),
  next_attempt_at timestamptz not null default now(),
  claimed_until timestamptz,
  error_message text,
  created_at timestamptz not null default now(),
  sent_at timestamptz,
  unique (notification_key, recipient)
);
create index alert_notifications_due on alert_notifications (next_attempt_at, created_at)
  where status in ('pending', 'sending');
create index alert_notifications_recent on alert_notifications (created_at desc, id desc);
