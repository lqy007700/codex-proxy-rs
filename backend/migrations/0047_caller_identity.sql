-- 调用方身份配置与稳定来源
create table caller_identity_sources (
  id text primary key,
  name text not null check (length(name) between 1 and 128),
  enabled boolean not null default true,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
alter table client_api_keys add column identity_policy_json jsonb not null default '{"mode":"direct"}'::jsonb;
alter table client_api_keys add column identity_source_id text generated always as (identity_policy_json->>'sourceId') stored references caller_identity_sources(id);
create table affinity_operations (
  id text primary key,
  plan_json jsonb not null,
  actor_json jsonb not null,
  results_json jsonb not null default '{}'::jsonb,
  expires_at timestamptz not null,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
create index affinity_operations_expiry_idx on affinity_operations(expires_at);
create index affinity_operations_created_at_idx on affinity_operations(created_at desc,id desc);

-- 入口身份独立于可变的 Key 配置，旧记录保持未知
alter table model_requests add column caller_identity_json jsonb;
