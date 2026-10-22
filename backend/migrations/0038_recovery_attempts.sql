-- 普通账号恢复总次数包含首次尝试，保底账号独立尝试一次
alter table runtime_settings
    add column recovery_attempts bigint not null default 4
    check (recovery_attempts between 1 and 31);
