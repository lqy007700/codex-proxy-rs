-- 持久规则独立于可过期的 Redis 会话
create table newapi_token_affinity_default (
  singleton boolean primary key default true check (singleton),
  enabled boolean not null default false,
  request_limit bigint not null default 50 check (request_limit between 1 and 4294967295),
  target_account text,
  check (not enabled or target_account is not null)
);
insert into newapi_token_affinity_default (singleton) values (true);

create table newapi_token_affinity_overrides (
  source_id text not null references caller_identity_sources(id) on delete cascade,
  user_id text not null,
  token_id text not null,
  enabled boolean not null,
  request_limit bigint not null check (request_limit between 1 and 4294967295),
  target_account text,
  primary key (source_id, user_id, token_id),
  check (not enabled or target_account is not null)
);
