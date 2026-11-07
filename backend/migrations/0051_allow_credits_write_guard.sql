-- 保持 allowCredits 关闭时的额度访问状态不可被延迟观测写成可用
create function apply_allow_credits_policy() returns trigger language plpgsql as $$
begin
  if not new.allow_credits and new.quota_evidence = 'credits_available' then
    new.quota_access_state := 'exhausted';
  end if;
  return new;
end
$$;

create trigger provider_accounts_allow_credits_policy
before insert or update of allow_credits, quota_evidence, quota_access_state on provider_accounts
for each row execute function apply_allow_credits_policy();
