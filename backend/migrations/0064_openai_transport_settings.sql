-- 缺省覆盖继续沿用部署设置，保留原有请求国家与 Guardian 设置
alter table runtime_settings
  add column openai_request_settings_json jsonb
    check (jsonb_typeof(openai_request_settings_json) = 'object'),
  add column openai_websocket_pool_json jsonb
    check (jsonb_typeof(openai_websocket_pool_json) = 'object');
