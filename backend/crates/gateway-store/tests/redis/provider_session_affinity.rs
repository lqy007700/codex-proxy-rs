//! 验证会话绑定的并发认领、版本比较、过期和存储损坏边界

use std::time::Duration;

use gateway_core::account::ProviderAccountId;
use gateway_core::provider_ports::{ProviderSessionAffinityKey, ProviderSessionAffinityPort};
use gateway_core::routing::ProviderKind;
use gateway_store::redis::RedisProviderSessionAffinityRepository;
use redis::aio::ConnectionManager;
use uuid::Uuid;

#[tokio::test]
async fn concurrent_claims_admit_exactly_one_account() {
    let Some((repo, mut connection, namespace)) = affinity_repository().await else {
        return;
    };
    let provider = ProviderKind::new("openai").unwrap();
    let key = ProviderSessionAffinityKey::try_new("secret-session").unwrap();
    let first = ProviderAccountId::new("acct_first").unwrap();
    let second = ProviderAccountId::new("acct_second").unwrap();
    let (a, b) = tokio::join!(
        repo.compare_and_bind(&provider, &key, None, &first, Duration::from_secs(60)),
        repo.compare_and_bind(&provider, &key, None, &second, Duration::from_secs(60))
    );
    let winners = [a.unwrap(), b.unwrap()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    assert_eq!(winners.len(), 1);
    assert_eq!(
        repo.load(&provider, &key).await.unwrap().as_ref(),
        winners.first()
    );
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg(format!("{namespace}:*"))
        .query_async(&mut connection)
        .await
        .unwrap();
    assert_eq!(keys.len(), 1);
    assert!(!keys[0].contains("secret-session"));
    let ttl: i64 = redis::cmd("PTTL")
        .arg(&keys[0])
        .query_async(&mut connection)
        .await
        .unwrap();
    assert!((1..=60_000).contains(&ttl));
}

#[tokio::test]
async fn stale_revision_cannot_overwrite_an_account_that_returned_to_the_session() {
    let Some((repo, _, _)) = affinity_repository().await else {
        return;
    };
    let provider = ProviderKind::new("openai").unwrap();
    let key = ProviderSessionAffinityKey::try_new("aba-session").unwrap();
    let a = ProviderAccountId::new("acct_a").unwrap();
    let b = ProviderAccountId::new("acct_b").unwrap();
    let ttl = Duration::from_secs(60);
    let first = repo
        .compare_and_bind(&provider, &key, None, &a, ttl)
        .await
        .unwrap()
        .unwrap();
    let second = repo
        .compare_and_bind(&provider, &key, Some(&first), &b, ttl)
        .await
        .unwrap()
        .unwrap();
    let third = repo
        .compare_and_bind(&provider, &key, Some(&second), &a, ttl)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(first.revision(), third.revision());
    assert!(
        repo.compare_and_bind(&provider, &key, Some(&first), &b, ttl)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(repo.load(&provider, &key).await.unwrap(), Some(third));
}

#[tokio::test]
async fn renewal_preserves_revision_but_expiration_requires_fresh_claim() {
    let Some((repo, mut connection, namespace)) = affinity_repository().await else {
        return;
    };
    let provider = ProviderKind::new("openai").unwrap();
    let key = ProviderSessionAffinityKey::try_new("expired-session").unwrap();
    let a = ProviderAccountId::new("acct_a").unwrap();
    let ttl = Duration::from_secs(60);
    let first = repo
        .compare_and_bind(&provider, &key, None, &a, ttl)
        .await
        .unwrap()
        .unwrap();
    let renewed = repo
        .compare_and_bind(&provider, &key, Some(&first), &a, ttl)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first, renewed);
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg(format!("{namespace}:*"))
        .query_async(&mut connection)
        .await
        .unwrap();
    let _: bool = redis::cmd("PEXPIRE")
        .arg(&keys[0])
        .arg(0)
        .query_async(&mut connection)
        .await
        .unwrap();
    assert!(
        repo.compare_and_bind(&provider, &key, Some(&first), &a, ttl)
            .await
            .unwrap()
            .is_none()
    );
    assert!(repo.load(&provider, &key).await.unwrap().is_none());
    let fresh = repo
        .compare_and_bind(&provider, &key, None, &a, ttl)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(first.revision(), fresh.revision());
}

#[tokio::test]
async fn damaged_binding_is_an_error_and_cannot_be_claimed_as_absent() {
    let Some((repo, mut connection, namespace)) = affinity_repository().await else {
        return;
    };
    let provider = ProviderKind::new("openai").unwrap();
    let key = ProviderSessionAffinityKey::try_new("damaged-session").unwrap();
    let a = ProviderAccountId::new("acct_a").unwrap();
    let ttl = Duration::from_secs(60);
    repo.compare_and_bind(&provider, &key, None, &a, ttl)
        .await
        .unwrap()
        .unwrap();
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg(format!("{namespace}:*"))
        .query_async(&mut connection)
        .await
        .unwrap();
    let _: () = redis::cmd("SET")
        .arg(&keys[0])
        .arg("invalid-binding")
        .query_async(&mut connection)
        .await
        .unwrap();
    assert!(repo.load(&provider, &key).await.is_err());
    assert!(
        repo.compare_and_bind(&provider, &key, None, &a, ttl)
            .await
            .unwrap()
            .is_none()
    );
}

async fn affinity_repository() -> Option<(
    RedisProviderSessionAffinityRepository,
    ConnectionManager,
    String,
)> {
    let redis_url = crate::support::test_env("CPR_TEST_REDIS_URL")?;
    let connection = redis::Client::open(redis_url)
        .unwrap()
        .get_connection_manager()
        .await
        .unwrap();
    let namespace = format!("gateway-store-affinity-test-{}", Uuid::new_v4());
    let repository =
        RedisProviderSessionAffinityRepository::new(connection.clone(), &namespace).unwrap();
    Some((repository, connection, namespace))
}

#[tokio::test]
async fn turn_alias_cannot_be_reassigned_and_follows_session_migration() {
    let Some((repo, _, _)) = affinity_repository().await else {
        return;
    };
    let provider = ProviderKind::new("openai").unwrap();
    let turn = ProviderSessionAffinityKey::try_new("client-turn").unwrap();
    let session = ProviderSessionAffinityKey::try_new("client-session").unwrap();
    let other = ProviderSessionAffinityKey::try_new("other-session").unwrap();
    let a = ProviderAccountId::new("acct_a").unwrap();
    let b = ProviderAccountId::new("acct_b").unwrap();
    let ttl = Duration::from_secs(60);
    let alias = gateway_core::provider_ports::ProviderSessionAlias {
        root_session_key: None,
        session_key: session.clone(),
        follow_only: true,
    };
    let other_alias = gateway_core::provider_ports::ProviderSessionAlias {
        root_session_key: None,
        session_key: other,
        follow_only: true,
    };
    assert!(
        repo.bind_alias(&provider, &turn, &alias, ttl)
            .await
            .unwrap()
    );
    assert!(
        !repo
            .bind_alias(&provider, &turn, &other_alias, ttl)
            .await
            .unwrap()
    );
    let different_role = gateway_core::provider_ports::ProviderSessionAlias {
        root_session_key: None,
        session_key: session.clone(),
        follow_only: false,
    };
    assert!(
        !repo
            .bind_alias(&provider, &turn, &different_role, ttl)
            .await
            .unwrap()
    );
    let first = repo
        .compare_and_bind(&provider, &session, None, &a, ttl)
        .await
        .unwrap()
        .unwrap();
    repo.compare_and_bind(&provider, &session, Some(&first), &b, ttl)
        .await
        .unwrap()
        .unwrap();
    let target = repo.load_alias(&provider, &turn).await.unwrap().unwrap();
    assert!(target.follow_only);
    assert_eq!(
        repo.load(&provider, &target.session_key)
            .await
            .unwrap()
            .unwrap()
            .account_id(),
        &b
    );
}

#[tokio::test]
async fn child_turn_alias_preserves_root_identity_without_allowing_reassignment() {
    let Some((repo, _, _)) = affinity_repository().await else {
        return;
    };
    let provider = ProviderKind::new("openai").unwrap();
    let turn = ProviderSessionAffinityKey::try_new("child-turn").unwrap();
    let alias = gateway_core::provider_ports::ProviderSessionAlias {
        session_key: ProviderSessionAffinityKey::try_new("child-session").unwrap(),
        root_session_key: Some(ProviderSessionAffinityKey::try_new("root-session").unwrap()),
        follow_only: false,
    };
    let ttl = Duration::from_secs(60);
    assert!(
        repo.bind_alias(&provider, &turn, &alias, ttl)
            .await
            .unwrap()
    );
    assert_eq!(
        repo.load_alias(&provider, &turn).await.unwrap(),
        Some(alias.clone())
    );
    let changed = gateway_core::provider_ports::ProviderSessionAlias {
        root_session_key: Some(ProviderSessionAffinityKey::try_new("other-root").unwrap()),
        ..alias
    };
    assert!(
        !repo
            .bind_alias(&provider, &turn, &changed, ttl)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn binding_and_turn_alias_can_renew_beyond_one_day_and_shorten_without_scanning() {
    let Some((repo, mut connection, namespace)) = affinity_repository().await else {
        return;
    };
    let provider = ProviderKind::new("openai").unwrap();
    let session = ProviderSessionAffinityKey::try_new("long-session").unwrap();
    let turn = ProviderSessionAffinityKey::try_new("long-turn").unwrap();
    let account = ProviderAccountId::new("acct_long").unwrap();
    let alias = gateway_core::provider_ports::ProviderSessionAlias {
        session_key: session.clone(),
        root_session_key: None,
        follow_only: false,
    };
    let mut binding = None;
    for hours in [24, 168, 720, 1] {
        let ttl = Duration::from_secs(hours * 3600);
        binding = repo
            .compare_and_bind(&provider, &session, binding.as_ref(), &account, ttl)
            .await
            .unwrap();
        assert!(binding.is_some());
        assert!(
            repo.bind_alias(&provider, &turn, &alias, ttl)
                .await
                .unwrap()
        );
        let keys: Vec<String> = redis::cmd("KEYS")
            .arg(format!("{namespace}:*"))
            .query_async(&mut connection)
            .await
            .unwrap();
        assert_eq!(keys.len(), 2);
        for key in keys {
            let remaining: u64 = redis::cmd("PTTL")
                .arg(key)
                .query_async(&mut connection)
                .await
                .unwrap();
            let expected = hours * 3600 * 1000;
            assert!(
                (expected - 5000..=expected).contains(&remaining),
                "{remaining} vs {expected}"
            );
        }
    }
    for ttl in [Duration::ZERO, Duration::from_secs(721 * 3600)] {
        assert!(
            repo.compare_and_bind(&provider, &session, binding.as_ref(), &account, ttl)
                .await
                .is_err()
        );
        assert!(
            repo.bind_alias(&provider, &turn, &alias, ttl)
                .await
                .is_err()
        );
    }
    assert_eq!(repo.load(&provider, &session).await.unwrap(), binding);
}

#[tokio::test]
async fn management_directives_are_versioned_idempotent_and_preserve_remaining_ttl() {
    use gateway_admin::{model::affinity::*, ports::affinity::AffinityBindingStore as _};
    use gateway_core::{
        policy::{CallerIdentity, ClientApiKeyId},
        provider_ports::{ProviderSessionObservation, SessionBindingDirective},
    };
    let Some((repo, _, _)) = affinity_repository().await else {
        return;
    };
    let provider = ProviderKind::new("openai").unwrap();
    for directive in [
        SessionBindingDirective::Clear,
        SessionBindingDirective::Reassign,
        SessionBindingDirective::Target {
            account_id: "acct_new".into(),
        },
    ] {
        let key = ProviderSessionAffinityKey::try_new(Uuid::new_v4().to_string()).unwrap();
        let account = ProviderAccountId::new("acct_old").unwrap();
        let old = repo
            .compare_and_bind(&provider, &key, None, &account, Duration::from_secs(60))
            .await
            .unwrap()
            .unwrap();
        repo.observe(
            &provider,
            &key,
            ProviderSessionObservation {
                token_name: None,
                binding_scope: Default::default(),
                caller: CallerIdentity::direct(&ClientApiKeyId::new("key_a").unwrap()),
                client_key_id: "key_a".into(),
                request_id: "req_observed".into(),
                actual_account_id: "acct_temporary".into(),
            },
            Duration::from_secs(60),
        )
        .await
        .unwrap();
        let session = repo
            .session("openai", key.expose_to_store())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(session.account_id, "acct_old");
        assert_eq!(session.observation.actual_account_id, "acct_temporary");
        let item = AffinityPlanItem {
            session: session.clone(),
            next_revision: Uuid::new_v4().simple().to_string(),
            rejection: None,
        };
        let plan = AffinityPlan {
            operation_id: Uuid::new_v4().to_string(),
            directive: directive.clone(),
            items: vec![item.clone()],
            created_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::minutes(15),
        };
        let applied = repo.apply(&plan, &item, None).await.unwrap();
        assert_eq!(applied.status, "applied");
        let pending = repo.load(&provider, &key).await.unwrap().unwrap();
        assert_eq!(pending.directive(), Some(&directive));
        assert_ne!(pending.revision(), old.revision());
        assert!(
            repo.compare_and_bind(
                &provider,
                &key,
                Some(&old),
                &account,
                Duration::from_secs(60)
            )
            .await
            .unwrap()
            .is_none(),
            "in-flight old admission cannot resurrect binding"
        );
        let after = repo
            .session("openai", key.expose_to_store())
            .await
            .unwrap()
            .unwrap();
        assert!(after.expires_at <= session.expires_at + chrono::Duration::milliseconds(20));
        let selected = ProviderAccountId::new("acct_new").unwrap();
        let consumed = repo
            .compare_and_bind(
                &provider,
                &key,
                Some(&pending),
                &selected,
                Duration::from_secs(60),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(consumed.directive().is_none());
        assert_ne!(consumed.revision(), pending.revision());
        assert_eq!(
            repo.apply(&plan, &item, Some("eligibility revoked after mutation"))
                .await
                .unwrap()
                .status,
            "applied"
        );
        assert_eq!(repo.load(&provider, &key).await.unwrap(), Some(consumed));
        assert_eq!(
            repo.receipt(&plan, &item).await.unwrap().unwrap().status,
            "applied"
        );
        let conflict_plan = AffinityPlan {
            operation_id: Uuid::new_v4().to_string(),
            ..plan.clone()
        };
        assert_eq!(
            repo.apply(&conflict_plan, &item, None)
                .await
                .unwrap()
                .status,
            "conflict"
        );
        let expired = AffinityPlan {
            operation_id: Uuid::new_v4().to_string(),
            expires_at: chrono::Utc::now() - chrono::Duration::seconds(1),
            ..plan
        };
        assert_eq!(
            repo.apply(&expired, &item, None).await.unwrap().status,
            "rejected"
        );
    }
    let page = repo
        .list(&AffinityQuery {
            request_id: Some("req_observed".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(page.items.len(), 3);
    assert_eq!(page.coverage, "observedOnly");
    assert!(
        repo.list(&AffinityQuery {
            user_id: Some("1".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .items
        .is_empty()
    );
}

#[tokio::test]
async fn missing_affinity_index_or_metadata_is_unavailable_and_unidentified_is_not_indexed() {
    use gateway_admin::{
        model::affinity::AffinityQuery, ports::affinity::AffinityBindingStore as _,
    };
    use gateway_core::{policy::CallerIdentity, provider_ports::ProviderSessionObservation};
    let Some((repo, mut connection, namespace)) = affinity_repository().await else {
        return;
    };
    let provider = ProviderKind::new("openai").unwrap();
    let key = ProviderSessionAffinityKey::try_new("index-state").unwrap();
    repo.compare_and_bind(
        &provider,
        &key,
        None,
        &ProviderAccountId::new("acct_a").unwrap(),
        Duration::from_secs(60),
    )
    .await
    .unwrap();
    let mut observation = ProviderSessionObservation {
        token_name: None,
        binding_scope: Default::default(),
        caller: CallerIdentity::Unidentified {
            request_id: "probe".into(),
        },
        client_key_id: "key_a".into(),
        request_id: "request".into(),
        actual_account_id: "acct_a".into(),
    };
    repo.observe(
        &provider,
        &key,
        observation.clone(),
        Duration::from_secs(60),
    )
    .await
    .unwrap();
    assert!(
        repo.list(&AffinityQuery::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    observation.caller = CallerIdentity::NewApi {
        source_id: "source".into(),
        user_id: "1".into(),
        token_id: "2".into(),
    };
    repo.observe(&provider, &key, observation, Duration::from_secs(60))
        .await
        .unwrap();
    assert_eq!(
        repo.list(&AffinityQuery::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    redis::cmd("DEL")
        .arg(format!("{namespace}:scheduler:affinity-index"))
        .query_async::<i64>(&mut connection)
        .await
        .unwrap();
    assert!(repo.list(&AffinityQuery::default()).await.is_err());
    let metadata: Vec<String> = redis::cmd("KEYS")
        .arg(format!("{namespace}:scheduler:session-binding:*:metadata"))
        .query_async(&mut connection)
        .await
        .unwrap();
    redis::cmd("DEL")
        .arg(metadata)
        .query_async::<i64>(&mut connection)
        .await
        .unwrap();
    assert!(repo.session("openai", key.expose_to_store()).await.is_err());
}

#[tokio::test]
async fn token_primary_and_session_observations_are_independently_filterable() {
    use gateway_admin::{
        model::affinity::AffinityQuery, ports::affinity::AffinityBindingStore as _,
    };
    use gateway_core::{
        policy::CallerIdentity,
        provider_ports::{ProviderAffinityScope, ProviderSessionObservation},
    };
    let Some((repo, _, _)) = affinity_repository().await else {
        return;
    };
    let provider = ProviderKind::new("openai").unwrap();
    for (name, scope) in [
        ("token", ProviderAffinityScope::Token),
        ("session", ProviderAffinityScope::Session),
    ] {
        let key = ProviderSessionAffinityKey::try_new(name).unwrap();
        repo.compare_and_bind(
            &provider,
            &key,
            None,
            &ProviderAccountId::new("acct_primary").unwrap(),
            Duration::from_secs(60),
        )
        .await
        .unwrap()
        .unwrap();
        repo.observe(
            &provider,
            &key,
            ProviderSessionObservation {
                token_name: None,
                binding_scope: scope,
                caller: CallerIdentity::NewApi {
                    source_id: "source".into(),
                    user_id: "1".into(),
                    token_id: "2".into(),
                },
                client_key_id: "key_shared".into(),
                request_id: "req_spillover".into(),
                actual_account_id: "acct_spillover".into(),
            },
            Duration::from_secs(60),
        )
        .await
        .unwrap();
    }
    for scope in [ProviderAffinityScope::Token, ProviderAffinityScope::Session] {
        let page = repo
            .list(&AffinityQuery {
                binding_scope: Some(scope),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].observation.binding_scope, scope);
        assert_eq!(page.items[0].account_id, "acct_primary");
        assert_eq!(
            page.items[0].observation.actual_account_id,
            "acct_spillover"
        );
    }
}

#[tokio::test]
async fn token_name_sidecar_preserves_names_and_binding_contract_while_ignoring_corruption() {
    use gateway_admin::{
        model::affinity::{AffinityPlan, AffinityPlanItem, AffinitySession},
        ports::affinity::AffinityBindingStore as _,
    };
    use gateway_core::{
        policy::CallerIdentity,
        provider_ports::{
            ProviderAffinityScope, ProviderSessionObservation, SessionBindingDirective,
        },
    };
    let Some((repo, mut connection, namespace)) = affinity_repository().await else {
        return;
    };
    let provider = ProviderKind::new("openai").unwrap();
    let key = ProviderSessionAffinityKey::try_new("named-token").unwrap();
    let binding = repo
        .compare_and_bind(
            &provider,
            &key,
            None,
            &ProviderAccountId::new("acct_primary").unwrap(),
            Duration::from_secs(60),
        )
        .await
        .unwrap()
        .unwrap();
    let mut observation = ProviderSessionObservation {
        token_name: Some(" 开发用 Key ".into()),
        binding_scope: ProviderAffinityScope::Token,
        caller: CallerIdentity::NewApi {
            source_id: "source".into(),
            user_id: "1".into(),
            token_id: "2".into(),
        },
        client_key_id: "key_shared".into(),
        request_id: "req_name".into(),
        actual_account_id: "acct_spillover".into(),
    };
    repo.observe(
        &provider,
        &key,
        observation.clone(),
        Duration::from_secs(60),
    )
    .await
    .unwrap();
    let metadata: Vec<String> = redis::cmd("KEYS")
        .arg(format!("{namespace}:scheduler:session-binding:*:metadata"))
        .query_async(&mut connection)
        .await
        .unwrap();
    assert_eq!(metadata.len(), 1);
    let binding_key = metadata[0].strip_suffix(":metadata").unwrap();
    let name_key = format!("{binding_key}:token-name");
    let session = repo
        .session("openai", key.expose_to_store())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        session.observation.token_name.as_deref(),
        Some("开发用 Key")
    );
    assert_eq!(session.account_id, "acct_primary");
    assert_eq!(session.observation.actual_account_id, "acct_spillover");
    let record: String = redis::cmd("GET")
        .arg(&metadata[0])
        .query_async(&mut connection)
        .await
        .unwrap();
    assert!(!record.contains("tokenName"));
    assert!(!record.contains("开发用 Key"));
    // 名称不进入持久化操作快照，旧版本的严格反序列化合同保持不变
    let plan = AffinityPlan {
        operation_id: Uuid::new_v4().to_string(),
        directive: SessionBindingDirective::Clear,
        items: vec![AffinityPlanItem {
            session: session.clone(),
            next_revision: "next".into(),
            rejection: None,
        }],
        created_at: chrono::Utc::now(),
        expires_at: chrono::Utc::now() + chrono::Duration::seconds(60),
    };
    let encoded = serde_json::to_string(&plan).unwrap();
    assert!(!encoded.contains("tokenName"));
    let restored: AffinityPlan = serde_json::from_str(&encoded).unwrap();
    assert_eq!(restored.items[0].session.revision, session.revision);
    assert!(restored.items[0].session.observation.token_name.is_none());
    let restored_session: AffinitySession =
        serde_json::from_value(serde_json::to_value(&session).unwrap()).unwrap();
    assert!(restored_session.observation.token_name.is_none());

    for name in [
        None,
        Some("   ".into()),
        Some("bad\tname".into()),
        Some("x".repeat(257)),
    ] {
        redis::cmd("PEXPIRE")
            .arg(&name_key)
            .arg(1_000)
            .query_async::<i64>(&mut connection)
            .await
            .unwrap();
        observation.token_name = name;
        repo.observe(
            &provider,
            &key,
            observation.clone(),
            Duration::from_secs(60),
        )
        .await
        .unwrap();
        let observed = repo
            .session("openai", key.expose_to_store())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            observed.observation.token_name.as_deref(),
            Some("开发用 Key")
        );
        assert_eq!(
            repo.load(&provider, &key).await.unwrap(),
            Some(binding.clone())
        );
        let name_ttl: i64 = redis::cmd("PTTL")
            .arg(&name_key)
            .query_async(&mut connection)
            .await
            .unwrap();
        let binding_ttl: i64 = redis::cmd("PTTL")
            .arg(binding_key)
            .query_async(&mut connection)
            .await
            .unwrap();
        assert!(name_ttl > 1_000 && name_ttl <= binding_ttl + 20);
    }
    observation.token_name = Some("重命名".into());
    repo.observe(
        &provider,
        &key,
        observation.clone(),
        Duration::from_secs(60),
    )
    .await
    .unwrap();
    assert_eq!(
        repo.session("openai", key.expose_to_store())
            .await
            .unwrap()
            .unwrap()
            .observation
            .token_name
            .as_deref(),
        Some("重命名")
    );
    redis::cmd("SET")
        .arg(&name_key)
        .arg(vec![0xff_u8])
        .query_async::<()>(&mut connection)
        .await
        .unwrap();
    assert!(
        repo.session("openai", key.expose_to_store())
            .await
            .unwrap()
            .unwrap()
            .observation
            .token_name
            .is_none()
    );
    redis::cmd("DEL")
        .arg(&name_key)
        .query_async::<i64>(&mut connection)
        .await
        .unwrap();
    redis::cmd("LPUSH")
        .arg(&name_key)
        .arg("wrong-type")
        .query_async::<i64>(&mut connection)
        .await
        .unwrap();
    observation.token_name = None;
    repo.observe(
        &provider,
        &key,
        observation.clone(),
        Duration::from_secs(60),
    )
    .await
    .unwrap();
    assert!(
        repo.session("openai", key.expose_to_store())
            .await
            .unwrap()
            .unwrap()
            .observation
            .token_name
            .is_none()
    );
    observation.token_name = Some("恢复名称".into());
    repo.observe(
        &provider,
        &key,
        observation.clone(),
        Duration::from_secs(60),
    )
    .await
    .unwrap();
    assert_eq!(
        repo.session("openai", key.expose_to_store())
            .await
            .unwrap()
            .unwrap()
            .observation
            .token_name
            .as_deref(),
        Some("恢复名称")
    );

    observation.caller = CallerIdentity::Direct {
        key_id: "key_shared".into(),
    };
    observation.token_name = Some("伪造名称".into());
    repo.observe(&provider, &key, observation, Duration::from_secs(60))
        .await
        .unwrap();
    assert!(
        repo.session("openai", key.expose_to_store())
            .await
            .unwrap()
            .unwrap()
            .observation
            .token_name
            .is_none()
    );
    assert_eq!(repo.load(&provider, &key).await.unwrap(), Some(binding));
}
