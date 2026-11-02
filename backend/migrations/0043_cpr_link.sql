-- 互联权限默认关闭，既有 Key 保持原来的访问范围
alter table client_api_keys
    add column peer_sharing_json jsonb not null
    default '{"enabled":false,"sharePool":false,"shareQuota":false,"sharePlans":false}';

-- 复用账号额度快照保存互联观测，网络响应仍由协议限制为 4 MiB
alter table provider_accounts drop constraint provider_accounts_quota_ck;
alter table provider_accounts add constraint provider_accounts_quota_ck check (
    provider_quota_json is null or (
        jsonb_typeof(provider_quota_json) = 'object'
        and octet_length(provider_quota_json::text) <= 8388608
    )
);
