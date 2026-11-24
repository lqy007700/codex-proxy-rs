alter table runtime_settings
    add column proxy_test_targets_json jsonb not null default '[]'::jsonb,
    add column proxy_test_targets_revision bigint not null default 1;

alter table outbound_proxies
    add column last_test_targets_json jsonb not null default '[]'::jsonb;
