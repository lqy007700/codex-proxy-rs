-- 分组生图权限默认允许，保留已有分组及未显式设置权限的创建行为
ALTER TABLE account_groups
    ADD COLUMN allow_image_generation boolean NOT NULL DEFAULT true;

-- 删除自动回切配置，保留既有智能调度系数
UPDATE runtime_settings
SET smart_scheduling_json = smart_scheduling_json - 'preferHigherWeight'
WHERE smart_scheduling_json ? 'preferHigherWeight';

ALTER TABLE runtime_settings
    ALTER COLUMN smart_scheduling_json SET DEFAULT
    '{"loadWeight":1.0,"quotaWeight":0.8,"healthWeight":1.0,"latencyWeight":0.5,"resetWeight":0.0,"queueWeight":0.0}'::jsonb;

-- 初始账号并发上限为 5，已保存的运行设置保持原值
alter table runtime_settings alter column max_concurrent_per_account set default 5;
update runtime_settings set max_concurrent_per_account = 5
where config_revision = 1 and max_concurrent_per_account = 3;
