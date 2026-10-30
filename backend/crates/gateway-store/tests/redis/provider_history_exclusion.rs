//! 验证历史排除指纹的真实 Redis 原子合并、隔离、过期与容量边界

use std::{collections::BTreeSet, time::Duration};

use gateway_core::provider_ports::{
    MAX_HISTORY_EXCLUSIONS, ProviderHistoryExclusionPort, ProviderSessionAffinityKey,
};
use gateway_store::redis::RedisProviderHistoryExclusionRepository;
use redis::aio::ConnectionManager;
use uuid::Uuid;

fn values(start: usize, end: usize) -> BTreeSet<String> {
    (start..end).map(|index| format!("{index:064x}")).collect()
}

async fn repository() -> Option<(
    RedisProviderHistoryExclusionRepository,
    ConnectionManager,
    String,
)> {
    let url = crate::support::test_env("CPR_TEST_REDIS_URL")?;
    let connection = redis::Client::open(url)
        .unwrap()
        .get_connection_manager()
        .await
        .unwrap();
    let namespace = format!("gateway-history-test-{}", Uuid::new_v4());
    Some((
        RedisProviderHistoryExclusionRepository::new(connection.clone(), &namespace).unwrap(),
        connection,
        namespace,
    ))
}

#[tokio::test]
async fn history_exclusion_merge_is_atomic_bounded_and_deduplicated() {
    let Some((repository, mut connection, namespace)) = repository().await else {
        return;
    };
    let scope = ProviderSessionAffinityKey::try_new("opaque-target-scope").unwrap();
    let ttl = Duration::from_secs(60);
    let a = values(0, 2048);
    let b = values(1024, MAX_HISTORY_EXCLUSIONS);
    let (a_result, b_result) = tokio::join!(
        repository.merge(&scope, &a, ttl),
        repository.merge(&scope, &b, ttl)
    );
    assert!(a_result.unwrap() && b_result.unwrap());
    let all = values(0, MAX_HISTORY_EXCLUSIONS);
    assert_eq!(repository.matching(&scope, &all, ttl).await.unwrap(), all);
    assert!(
        !repository
            .merge(&scope, &values(4095, 4098), ttl)
            .await
            .unwrap()
    );
    assert!(
        repository
            .matching(&scope, &values(4096, 4098), ttl)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(repository.merge(&scope, &values(0, 1), ttl).await.unwrap());
    let other = ProviderSessionAffinityKey::try_new("another-target-scope").unwrap();
    assert!(
        repository
            .matching(&other, &all, ttl)
            .await
            .unwrap()
            .is_empty()
    );
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg(format!("{namespace}:*"))
        .query_async(&mut connection)
        .await
        .unwrap();
    assert_eq!(keys.len(), 1);
    assert!(!keys[0].contains("opaque-target-scope"));
    redis::cmd("DEL")
        .arg(keys)
        .query_async::<usize>(&mut connection)
        .await
        .unwrap();
}

#[tokio::test]
async fn history_exclusion_renews_only_on_match_and_expires() {
    let Some((repository, mut connection, namespace)) = repository().await else {
        return;
    };
    let scope = ProviderSessionAffinityKey::try_new("expiring-target").unwrap();
    let batch = values(1, 3);
    assert!(
        repository
            .matching(&scope, &batch, Duration::from_secs(60))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        repository
            .merge(&scope, &batch, Duration::from_secs(1))
            .await
            .unwrap()
    );
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg(format!("{namespace}:*"))
        .query_async(&mut connection)
        .await
        .unwrap();
    assert_eq!(keys.len(), 1);
    assert!(
        repository
            .matching(&scope, &values(10, 11), Duration::from_secs(3600))
            .await
            .unwrap()
            .is_empty()
    );
    let ttl: i64 = redis::cmd("PTTL")
        .arg(&keys[0])
        .query_async(&mut connection)
        .await
        .unwrap();
    assert!((1..=1000).contains(&ttl));
    assert_eq!(
        repository
            .matching(&scope, &batch, Duration::from_secs(3600))
            .await
            .unwrap(),
        batch
    );
    let ttl: i64 = redis::cmd("PTTL")
        .arg(&keys[0])
        .query_async(&mut connection)
        .await
        .unwrap();
    assert!((3_590_000..=3_600_000).contains(&ttl));
    redis::cmd("PEXPIRE")
        .arg(&keys[0])
        .arg(1)
        .query_async::<bool>(&mut connection)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(
        repository
            .matching(&scope, &batch, Duration::from_secs(60))
            .await
            .unwrap()
            .is_empty()
    );
}
