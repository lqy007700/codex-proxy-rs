-- 上游明确指出耗尽窗口时，随该次拒绝保留其时长
alter table provider_accounts
    add column quota_limit_window_minutes integer
    check (quota_limit_window_minutes between 1 and 65535);

-- 记录最近一次运行设置写入对应的全局发布版本，供编辑草稿判断冲突
alter table runtime_settings add column settings_revision bigint not null default 1;
update runtime_settings set settings_revision = config_revision;
alter table runtime_settings add constraint runtime_settings_edit_revision_ck
    check (settings_revision between 1 and config_revision);
