-- 账号用途独立于认证方式，已有账号继续参与常规调度
alter table provider_accounts
    add column scheduling_role text not null default 'regular'
    check (scheduling_role in ('regular', 'fallback'));
