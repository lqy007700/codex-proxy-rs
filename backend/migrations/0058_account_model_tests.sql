-- 账号模型测试记录与受认证访问的有界图片正文
create table account_model_tests (
  id uuid primary key,
  account_id text not null references provider_accounts (id) on delete cascade,
  account_name text not null,
  model_id text not null,
  kind text not null check (kind in ('text', 'image', 'animation')),
  prompt text not null check (octet_length(prompt) between 1 and 32768),
  started_at timestamptz not null,
  finished_at timestamptz,
  error text,
  result_text bytea,
  image_type text check (image_type in ('image/png', 'image/jpeg', 'image/webp', 'image/gif')),
  image_bytes bytea,
  check ((image_type is null) = (image_bytes is null)),
  check (num_nonnulls(error, result_text, image_bytes) = (finished_at is not null)::int)
);

create unique index account_model_tests_running_idx
  on account_model_tests (account_id) where finished_at is null;
create index account_model_tests_start_idx on account_model_tests (started_at desc, id desc);
create index account_model_tests_account_start_idx
  on account_model_tests (account_id, started_at desc, id desc);
