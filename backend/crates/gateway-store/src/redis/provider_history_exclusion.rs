//! 历史排除指纹的原子合并、匹配续期与容量限制，不保存协议正文

use std::{collections::BTreeSet, time::Duration};

use futures::future::BoxFuture;
use gateway_core::provider_ports::{
    MAX_HISTORY_EXCLUSIONS, ProviderHistoryExclusionPort, ProviderSessionAffinityKey,
    ProviderStoreError, ProviderStoreErrorKind,
};
use redis::aio::ConnectionManager;

use crate::StoreResult;

const MATCHING: &str = r#"
local result = {}
for i = 2, #ARGV do
  if redis.call('SISMEMBER', KEYS[1], ARGV[i]) == 1 then
    table.insert(result, ARGV[i])
  end
end
if #result > 0 then redis.call('EXPIRE', KEYS[1], ARGV[1]) end
return result
"#;

const MERGE: &str = r#"
local added = 0
for i = 3, #ARGV do
  if redis.call('SISMEMBER', KEYS[1], ARGV[i]) == 0 then added = added + 1 end
end
if redis.call('SCARD', KEYS[1]) + added > tonumber(ARGV[2]) then return 0 end
for i = 3, #ARGV do redis.call('SADD', KEYS[1], ARGV[i]) end
redis.call('EXPIRE', KEYS[1], ARGV[1])
return 1
"#;

#[derive(Clone)]
pub struct RedisProviderHistoryExclusionRepository {
    connection: ConnectionManager,
    namespace: String,
}

impl RedisProviderHistoryExclusionRepository {
    pub fn new(connection: ConnectionManager, namespace: &str) -> StoreResult<Self> {
        Ok(Self {
            connection,
            namespace: super::namespace(namespace)?,
        })
    }

    fn key(&self, scope: &ProviderSessionAffinityKey) -> Result<String, ProviderStoreError> {
        let fingerprint =
            super::resource_fingerprint("provider history scope", scope.expose_to_store())
                .map_err(|_| invalid())?;
        Ok(format!(
            "{}:provider:history-exclusion:v1:{{{fingerprint}}}",
            self.namespace
        ))
    }
}

fn invalid() -> ProviderStoreError {
    ProviderStoreError::new(
        ProviderStoreErrorKind::InvalidData,
        "invalid history fingerprint batch",
    )
}

fn validate(values: &BTreeSet<String>) -> Result<(), ProviderStoreError> {
    if values.len() > MAX_HISTORY_EXCLUSIONS
        || values
            .iter()
            .any(|value| value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(invalid());
    }
    Ok(())
}

fn ttl_seconds(ttl: Duration) -> u64 {
    ttl.as_secs().clamp(1, 24 * 60 * 60)
}

impl ProviderHistoryExclusionPort for RedisProviderHistoryExclusionRepository {
    fn matching<'a>(
        &'a self,
        scope: &'a ProviderSessionAffinityKey,
        candidates: &'a BTreeSet<String>,
        ttl: Duration,
    ) -> BoxFuture<'a, Result<BTreeSet<String>, ProviderStoreError>> {
        Box::pin(async move {
            validate(candidates)?;
            if candidates.is_empty() {
                return Ok(BTreeSet::new());
            }
            let mut connection = self.connection.clone();
            redis::Script::new(MATCHING)
                .key(self.key(scope)?)
                .arg(ttl_seconds(ttl))
                .arg(candidates.iter().collect::<Vec<_>>())
                .invoke_async(&mut connection)
                .await
                .map_err(|error| {
                    ProviderStoreError::caused_by(
                        ProviderStoreErrorKind::Unavailable,
                        "history exclusion cache",
                        error,
                    )
                })
        })
    }

    fn merge<'a>(
        &'a self,
        scope: &'a ProviderSessionAffinityKey,
        fingerprints: &'a BTreeSet<String>,
        ttl: Duration,
    ) -> BoxFuture<'a, Result<bool, ProviderStoreError>> {
        Box::pin(async move {
            validate(fingerprints)?;
            if fingerprints.is_empty() {
                return Ok(true);
            }
            let mut connection = self.connection.clone();
            redis::Script::new(MERGE)
                .key(self.key(scope)?)
                .arg(ttl_seconds(ttl))
                .arg(MAX_HISTORY_EXCLUSIONS)
                .arg(fingerprints.iter().collect::<Vec<_>>())
                .invoke_async(&mut connection)
                .await
                .map_err(|error| {
                    ProviderStoreError::caused_by(
                        ProviderStoreErrorKind::Unavailable,
                        "history exclusion cache",
                        error,
                    )
                })
        })
    }
}
