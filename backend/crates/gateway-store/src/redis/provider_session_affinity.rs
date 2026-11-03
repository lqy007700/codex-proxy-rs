//! 会话账号绑定的 Redis 版本快照、原子认领与续期

use std::time::Duration;

use gateway_core::account::ProviderAccountId;
use gateway_core::provider_ports::{
    ProviderSessionAffinityKey, ProviderSessionAffinityPort, ProviderSessionBinding,
    ProviderStoreError, ProviderStoreErrorKind,
};
use gateway_core::routing::ProviderKind;
use redis::aio::ConnectionManager;

use crate::StoreResult;

use super::{namespace, resource_fingerprint};

const MAX_SESSION_AFFINITY_TTL: Duration =
    Duration::from_secs(gateway_core::account::MAX_SESSION_AFFINITY_TTL_HOURS as u64 * 3600);

// 比较完整记录而非账号 ID；冲突时既不改绑定，也不续期
const COMPARE_AND_BIND_SCRIPT: &str = r#"
local current = redis.call('GET', KEYS[1])
if (not current and ARGV[1] == '') or current == ARGV[1] then
  redis.call('PSETEX', KEYS[1], tonumber(ARGV[3]), ARGV[2])
  return 1
end
return 0
"#;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BindingRecord {
    account_id: String,
    revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    directive: Option<gateway_core::provider_ports::SessionBindingDirective>,
}

impl BindingRecord {
    fn encode(binding: &ProviderSessionBinding) -> Result<String, ProviderStoreError> {
        serde_json::to_string(&Self {
            account_id: binding.account_id().as_str().to_owned(),
            revision: binding.revision().to_owned(),
            directive: binding.directive().cloned(),
        })
        .map_err(|_| provider_invalid("encode provider session binding"))
    }

    fn decode(raw: &str) -> Result<ProviderSessionBinding, ProviderStoreError> {
        let record: Self = serde_json::from_str(raw)
            .map_err(|_| provider_invalid("decode provider session binding"))?;
        let account = ProviderAccountId::new(record.account_id)
            .map_err(|_| provider_invalid("decode provider session binding account"))?;
        ProviderSessionBinding::new(account, record.revision)
            .map(|binding| binding.with_directive(record.directive))
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AliasRecord {
    session_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    root_session_key: Option<String>,
    follow_only: bool,
}

#[derive(Clone)]
pub struct RedisProviderSessionAffinityRepository {
    connection: ConnectionManager,
    namespace: String,
}

impl RedisProviderSessionAffinityRepository {
    pub fn new(connection: ConnectionManager, key_namespace: &str) -> StoreResult<Self> {
        Ok(Self {
            connection,
            namespace: namespace(key_namespace)?,
        })
    }

    fn key(
        &self,
        provider_kind: &ProviderKind,
        affinity_key: &ProviderSessionAffinityKey,
    ) -> Result<String, ProviderStoreError> {
        let scope = format!(
            "{}\0{}",
            provider_kind.as_str(),
            affinity_key.expose_to_store()
        );
        let fingerprint = resource_fingerprint("provider session affinity", &scope)
            .map_err(|_| provider_invalid("encode provider session affinity key"))?;
        Ok(format!(
            "{}:scheduler:session-binding:{{{fingerprint}}}",
            self.namespace
        ))
    }
}

impl ProviderSessionAffinityPort for RedisProviderSessionAffinityRepository {
    fn observe<'a>(
        &'a self,
        provider: &'a ProviderKind,
        key: &'a ProviderSessionAffinityKey,
        observation: gateway_core::provider_ports::ProviderSessionObservation,
        _ttl: Duration,
    ) -> futures::future::BoxFuture<'a, Result<(), ProviderStoreError>> {
        Box::pin(async move {
            if observation.caller.is_unidentified() {
                return Ok(());
            }
            let record = serde_json::to_string(&IndexRecord {
                observation,
                first_observed_ms: 0,
                last_admission_ms: 0,
            })
            .map_err(|_| provider_invalid("encode affinity index"))?;
            let binding_key = self.key(provider, key)?;
            redis::Script::new(OBSERVE_SCRIPT)
                .key(&binding_key)
                .key(format!("{binding_key}:metadata"))
                .key(self.index_key())
                .key(format!("{}:initialized", self.index_key()))
                .key(format!("{}:expiry", self.index_key()))
                .arg(format!("{}:{}", provider.as_str(), key.expose_to_store()))
                .arg(record)
                .invoke_async::<i64>(&mut self.connection.clone())
                .await
                .map_err(|error| crate::provider_unavailable("observe affinity session", error))?;
            Ok(())
        })
    }

    fn load<'a>(
        &'a self,
        provider_kind: &'a ProviderKind,
        key: &'a ProviderSessionAffinityKey,
    ) -> futures::future::BoxFuture<'a, Result<Option<ProviderSessionBinding>, ProviderStoreError>>
    {
        Box::pin(async move {
            let raw = redis::cmd("GET")
                .arg(self.key(provider_kind, key)?)
                .query_async::<Option<String>>(&mut self.connection.clone())
                .await
                .map_err(|source| {
                    crate::provider_unavailable("load provider session binding", source)
                })?;
            raw.as_deref().map(BindingRecord::decode).transpose()
        })
    }

    fn compare_and_bind<'a>(
        &'a self,
        provider_kind: &'a ProviderKind,
        key: &'a ProviderSessionAffinityKey,
        expected: Option<&'a ProviderSessionBinding>,
        account_id: &'a ProviderAccountId,
        ttl: Duration,
    ) -> futures::future::BoxFuture<'a, Result<Option<ProviderSessionBinding>, ProviderStoreError>>
    {
        Box::pin(async move {
            let ttl_millis = session_affinity_ttl_millis(ttl)?;
            let expected_raw = expected
                .map(BindingRecord::encode)
                .transpose()?
                .unwrap_or_default();
            // 同账号命中仅续期；初次认领及换号生成新版本
            let binding = match expected.filter(|binding| {
                binding.account_id() == account_id && binding.directive().is_none()
            }) {
                Some(binding) => binding.clone(),
                None => ProviderSessionBinding::new(
                    account_id.clone(),
                    uuid::Uuid::new_v4().simple().to_string(),
                )?,
            };
            let applied = redis::Script::new(COMPARE_AND_BIND_SCRIPT)
                .key(self.key(provider_kind, key)?)
                .arg(expected_raw)
                .arg(BindingRecord::encode(&binding)?)
                .arg(ttl_millis)
                .invoke_async::<bool>(&mut self.connection.clone())
                .await
                .map_err(|source| {
                    crate::provider_unavailable("admit provider session binding", source)
                })?;
            Ok(applied.then_some(binding))
        })
    }
    fn load_alias<'a>(
        &'a self,
        provider: &'a ProviderKind,
        alias: &'a ProviderSessionAffinityKey,
    ) -> futures::future::BoxFuture<
        'a,
        Result<Option<gateway_core::provider_ports::ProviderSessionAlias>, ProviderStoreError>,
    > {
        Box::pin(async move {
            let value = redis::cmd("GET")
                .arg(format!("{}:alias", self.key(provider, alias)?))
                .query_async::<Option<String>>(&mut self.connection.clone())
                .await
                .map_err(|source| crate::provider_unavailable("load session alias", source))?;
            value
                .map(|raw| {
                    let record: AliasRecord = serde_json::from_str(&raw)
                        .map_err(|_| provider_invalid("decode session alias"))?;
                    Ok(gateway_core::provider_ports::ProviderSessionAlias {
                        session_key: ProviderSessionAffinityKey::try_new(record.session_key)?,
                        follow_only: record.follow_only,
                        root_session_key: record
                            .root_session_key
                            .map(ProviderSessionAffinityKey::try_new)
                            .transpose()?,
                    })
                })
                .transpose()
        })
    }

    fn bind_alias<'a>(
        &'a self,
        provider: &'a ProviderKind,
        alias: &'a ProviderSessionAffinityKey,
        session: &'a gateway_core::provider_ports::ProviderSessionAlias,
        ttl: Duration,
    ) -> futures::future::BoxFuture<'a, Result<bool, ProviderStoreError>> {
        Box::pin(async move {
            let record = serde_json::to_string(&AliasRecord {
                session_key: session.session_key.expose_to_store().to_owned(),
                follow_only: session.follow_only,
                root_session_key: session
                    .root_session_key
                    .as_ref()
                    .map(|key| key.expose_to_store().to_owned()),
            })
            .map_err(|_| provider_invalid("encode session alias"))?;
            redis::Script::new("local current = redis.call('GET', KEYS[1]); if not current or current == ARGV[1] then redis.call('PSETEX', KEYS[1], tonumber(ARGV[3]), ARGV[2]); return 1 end; return 0")
                .key(format!("{}:alias", self.key(provider, alias)?))
                // 缺失与同目标均可写入，冲突不能覆盖另一会话
                .arg(&record)
                .arg(&record)
                .arg(session_affinity_ttl_millis(ttl)?)
                .invoke_async::<bool>(&mut self.connection.clone()).await
                .map_err(|source| crate::provider_unavailable("bind session alias", source))
        })
    }
}

fn session_affinity_ttl_millis(ttl: Duration) -> Result<u64, ProviderStoreError> {
    if ttl.is_zero() || ttl > MAX_SESSION_AFFINITY_TTL {
        return Err(provider_invalid("validate provider session affinity TTL"));
    }
    u64::try_from(ttl.as_millis())
        .map_err(|_| provider_invalid("validate provider session affinity TTL"))
}

fn provider_invalid(operation: &'static str) -> ProviderStoreError {
    ProviderStoreError::new(ProviderStoreErrorKind::InvalidData, operation)
}

// 索引和观测字段不保存账号绑定，列表读取时始终复核 Redis 的版本事实
const OBSERVE_SCRIPT: &str = r#"
local binding = redis.call('GET', KEYS[1])
if not binding then return 0 end
local ttl = redis.call('PTTL', KEYS[1])
if ttl <= 0 then return 0 end
if redis.call('EXISTS', KEYS[4]) == 1 and not redis.call('ZSCORE', KEYS[3], '') then
  return redis.error_reply('affinity index incomplete')
end
local time = redis.call('TIME')
local now = tonumber(time[1])*1000+math.floor(tonumber(time[2])/1000)
local record = cjson.decode(ARGV[2])
local old = redis.call('GET', KEYS[2])
record.firstObservedMs = old and cjson.decode(old).firstObservedMs or now
record.lastAdmissionMs = now
redis.call('PSETEX', KEYS[2], ttl, cjson.encode(record))
redis.call('ZADD', KEYS[3], 0, '', 0, ARGV[1])
redis.call('SET', KEYS[4], '1')
redis.call('ZADD', KEYS[5], now+ttl, ARGV[1])
local expired=redis.call('ZRANGEBYSCORE', KEYS[5], '-inf', now, 'LIMIT', 0, 64)
for _, member in ipairs(expired) do
  redis.call('ZREM', KEYS[3], member)
  redis.call('ZREM', KEYS[5], member)
end
return 1
"#;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IndexRecord {
    observation: gateway_core::provider_ports::ProviderSessionObservation,
    first_observed_ms: i64,
    last_admission_ms: i64,
}

impl RedisProviderSessionAffinityRepository {
    fn index_key(&self) -> String {
        format!("{}:scheduler:affinity-index", self.namespace)
    }
}

fn admin_unavailable(error: impl std::fmt::Display) -> gateway_admin::model::AdminError {
    tracing::warn!(%error, "Affinity management state unavailable");
    gateway_admin::model::AdminError::new(
        gateway_admin::model::AdminErrorKind::Unavailable,
        "亲和索引或存储不可用，请稍后刷新",
    )
}

#[async_trait::async_trait]
impl gateway_admin::ports::affinity::AffinityBindingStore
    for RedisProviderSessionAffinityRepository
{
    async fn list(
        &self,
        query: &gateway_admin::model::affinity::AffinityQuery,
    ) -> Result<gateway_admin::model::affinity::AffinityPage, gateway_admin::model::AdminError>
    {
        let ids: Vec<String> = redis::Script::new(r#"
if redis.call('EXISTS', KEYS[2]) == 1 and not redis.call('ZSCORE', KEYS[1], '') then return redis.error_reply('affinity index incomplete') end
return redis.call('ZRANGEBYLEX', KEYS[1], ARGV[1], '+', 'LIMIT', 0, 201)
"#).key(self.index_key()).key(format!("{}:initialized", self.index_key())).arg(format!("({}", query.cursor.as_deref().unwrap_or_default())).invoke_async(&mut self.connection.clone()).await.map_err(admin_unavailable)?;
        let next_cursor = (ids.len() > 200).then(|| ids[199].clone());
        let mut items = Vec::new();
        for id in ids.iter().take(200) {
            let (provider, reference) = id
                .split_once(':')
                .ok_or_else(|| admin_unavailable("invalid affinity index member"))?;
            match self.session(provider, reference).await? {
                Some(session) if query.matches(&session) => items.push(session),
                Some(_) => {}
                None => {
                    redis::cmd("ZREM")
                        .arg(self.index_key())
                        .arg(id)
                        .query_async::<i64>(&mut self.connection.clone())
                        .await
                        .map_err(admin_unavailable)?;
                }
            }
        }
        Ok(gateway_admin::model::affinity::AffinityPage {
            items,
            next_cursor,
            coverage: "observedOnly",
        })
    }

    async fn session(
        &self,
        provider: &str,
        reference: &str,
    ) -> Result<
        Option<gateway_admin::model::affinity::AffinitySession>,
        gateway_admin::model::AdminError,
    > {
        let provider = ProviderKind::new(provider).map_err(admin_unavailable)?;
        let key = ProviderSessionAffinityKey::try_new(reference).map_err(admin_unavailable)?;
        let binding_key = self.key(&provider, &key).map_err(admin_unavailable)?;
        let values: Vec<String> = redis::Script::new(r#"
local binding=redis.call('GET', KEYS[1])
if not binding then return {} end
local metadata=redis.call('GET', KEYS[2])
if not metadata then return redis.error_reply('affinity metadata missing') end
local ttl=redis.call('PTTL', KEYS[1])
local time=redis.call('TIME')
return {binding, metadata, tostring(ttl), tostring(tonumber(time[1])*1000+math.floor(tonumber(time[2])/1000))}
"#).key(&binding_key).key(format!("{binding_key}:metadata")).invoke_async(&mut self.connection.clone()).await.map_err(admin_unavailable)?;
        if values.is_empty() {
            return Ok(None);
        }
        if values.len() != 4 {
            return Err(admin_unavailable("incomplete affinity snapshot"));
        }
        let binding = BindingRecord::decode(&values[0]).map_err(admin_unavailable)?;
        let record: IndexRecord = serde_json::from_str(&values[1]).map_err(admin_unavailable)?;
        let ttl = values[2].parse::<i64>().map_err(admin_unavailable)?;
        let now = values[3].parse::<i64>().map_err(admin_unavailable)?;
        if ttl <= 0 {
            return Ok(None);
        }
        let datetime = |value| {
            chrono::DateTime::from_timestamp_millis(value)
                .ok_or_else(|| admin_unavailable("invalid affinity timestamp"))
        };
        Ok(Some(gateway_admin::model::affinity::AffinitySession {
            reference: reference.to_owned(),
            provider: provider.as_str().to_owned(),
            observation: record.observation,
            account_id: binding.account_id().as_str().to_owned(),
            revision: binding.revision().to_owned(),
            directive: binding.directive().cloned(),
            first_observed_at: datetime(record.first_observed_ms)?,
            last_admission_at: datetime(record.last_admission_ms)?,
            expires_at: datetime(now.saturating_add(ttl))?,
        }))
    }

    async fn receipt(
        &self,
        plan: &gateway_admin::model::affinity::AffinityPlan,
        item: &gateway_admin::model::affinity::AffinityPlanItem,
    ) -> Result<
        Option<gateway_admin::model::affinity::AffinityItemResult>,
        gateway_admin::model::AdminError,
    > {
        let raw: Option<String> = redis::cmd("GET")
            .arg(self.operation_key(plan, item))
            .query_async(&mut self.connection.clone())
            .await
            .map_err(admin_unavailable)?;
        raw.map(|raw| serde_json::from_str(&raw).map_err(admin_unavailable))
            .transpose()
    }

    async fn apply(
        &self,
        plan: &gateway_admin::model::affinity::AffinityPlan,
        item: &gateway_admin::model::affinity::AffinityPlanItem,
        rejection: Option<&str>,
    ) -> Result<gateway_admin::model::affinity::AffinityItemResult, gateway_admin::model::AdminError>
    {
        use gateway_admin::model::affinity::AffinityItemResult;
        let session = &item.session;
        let provider = ProviderKind::new(&session.provider).map_err(admin_unavailable)?;
        let key =
            ProviderSessionAffinityKey::try_new(&session.reference).map_err(admin_unavailable)?;
        let account = ProviderAccountId::new(&session.account_id).map_err(admin_unavailable)?;
        let expected = ProviderSessionBinding::new(account.clone(), session.revision.clone())
            .map_err(admin_unavailable)?
            .with_directive(session.directive.clone());
        let replacement = ProviderSessionBinding::new(account, item.next_revision.clone())
            .map_err(admin_unavailable)?
            .with_directive(Some(plan.directive.clone()));
        let result = |status: &str, message: &str| {
            serde_json::to_string(&AffinityItemResult {
                provider: session.provider.clone(),
                reference: session.reference.clone(),
                status: status.to_owned(),
                message: message.to_owned(),
                audit_recorded: false,
            })
            .map_err(admin_unavailable)
        };
        let raw: String = redis::Script::new(r#"
local previous=redis.call('GET', KEYS[2])
if previous then return previous end
local time=redis.call('TIME')
local now=tonumber(time[1])*1000+math.floor(tonumber(time[2])/1000)
if now >= tonumber(ARGV[6]) then redis.call('SET', KEYS[2], ARGV[7], 'EX', 2592000); return ARGV[7] end
local current=redis.call('GET', KEYS[1])
local result=ARGV[4]
local ttl=redis.call('PTTL', KEYS[1])
if ARGV[5] ~= '' then
  result=ARGV[5]
elseif ttl > 0 and current == ARGV[1] then
  redis.call('PSETEX', KEYS[1], ttl, ARGV[2])
  result=ARGV[3]
end
redis.call('SET', KEYS[2], result, 'EX', 2592000)
return result
"#).key(self.key(&provider, &key).map_err(admin_unavailable)?).key(self.operation_key(plan, item))
            .arg(BindingRecord::encode(&expected).map_err(admin_unavailable)?).arg(BindingRecord::encode(&replacement).map_err(admin_unavailable)?)
            .arg(result("applied", "已更新，将在下一次安全选号时生效；原生续接不迁移账号")?)
            .arg(result("conflict", "绑定已变化或过期，请重新预览")?)
            .arg(rejection.map(|reason| result("rejected", reason)).transpose()?.unwrap_or_default())
            .arg(plan.expires_at.timestamp_millis()).arg(result("rejected", "预览已过期，未执行此项")?)
            .invoke_async(&mut self.connection.clone()).await.map_err(admin_unavailable)?;
        serde_json::from_str(&raw).map_err(admin_unavailable)
    }
}

impl RedisProviderSessionAffinityRepository {
    fn operation_key(
        &self,
        plan: &gateway_admin::model::affinity::AffinityPlan,
        item: &gateway_admin::model::affinity::AffinityPlanItem,
    ) -> String {
        format!(
            "{}:affinity-operation:{}:{}",
            self.namespace,
            plan.operation_id,
            item.key()
        )
    }
}
