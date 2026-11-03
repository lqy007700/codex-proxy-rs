-- 初始账号与其他普通账号分别计数，保留最后一次保底的路由预算
ALTER TABLE runtime_settings
    ADD COLUMN recovery_rotation_attempts BIGINT NOT NULL DEFAULT 1;

-- 保留已有初始账号极限值，不在升级时扩大请求总预算
UPDATE runtime_settings SET recovery_rotation_attempts = 0 WHERE recovery_attempts = 31;

ALTER TABLE runtime_settings
    ADD CONSTRAINT runtime_settings_recovery_rotation_attempts_check
    CHECK (recovery_rotation_attempts BETWEEN 0 AND 30
        AND recovery_attempts + recovery_rotation_attempts <= 31);
