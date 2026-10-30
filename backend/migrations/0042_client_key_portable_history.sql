-- Key 显式开启跨渠道历史兼容，已有 Key 保持原生回放
alter table client_api_keys
    add column portable_history boolean not null default false;
