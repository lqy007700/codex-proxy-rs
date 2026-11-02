-- 账号模型映射属于路由配置，不随凭据刷新或重新导入覆盖
alter table provider_accounts
  add column model_mappings_json jsonb not null default '{}'::jsonb,
  add constraint provider_accounts_model_mappings_ck check (
    jsonb_typeof(model_mappings_json) = 'object'
    and octet_length(model_mappings_json::text) <= 262144
  );
