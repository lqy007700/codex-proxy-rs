//! 验证额度暂停策略、快照合并与连接变更的事务隔离

use super::*;
use gateway_admin::{
    model::{
        accounts::{PreparedQuotaPause, PreparedQuotaPauses},
        proxies::{AccountProxySelection, UpdateProxy},
    },
    ports::proxy::ProxyStore,
};
use gateway_core::account::{
    AccountQuotaLimits, QuotaMergeBase, QuotaPauseDecision, UpstreamConnection,
};
use gateway_store::postgres::PgProxyRepository;

fn limits(percent: u8) -> AccountQuotaLimits {
    serde_json::from_value(json!({"codex:18000s": percent})).unwrap()
}

fn observation(id: &str, revision: u64, observed_at: SystemTime) -> QuotaObservation {
    QuotaObservation {
        account_id: ProviderAccountId::new(id).unwrap(),
        expected_revision: CredentialRevision::new(revision).unwrap(),
        expected_connection: None,
        quota: OpaqueProviderData::new(json!({"marker": "confirmed"}).as_object().unwrap().clone()),
        observed_at,
        state: QuotaState::unknown(),
        plan_type: None,
        quota_pause: None,
    }
}

fn batch(ids: &[&str], quota_limits: Option<AccountQuotaLimits>) -> BatchUpdateAccounts {
    BatchUpdateAccounts {
        account_ids: ids.iter().map(|id| (*id).to_owned()).collect(),
        quota_limits,
        enabled: None,
        allow_credits: None,
        concurrency_limit: None,
        weight: None,
        model_access: None,
        model_mappings: None,
        group_ids: None,
        outbound_proxy: None,
        scheduling_role: None,
    }
}

#[tokio::test]
async fn quota_pause_merge_requires_current_policy_and_exact_snapshot_version() {
    let Some(database) = TestDatabase::create("quota_pause_merge").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    let id = ProviderAccountId::new("acct_merge").unwrap();
    repository
        .insert_provider_account(account(id.as_str(), "merge"))
        .await
        .unwrap();
    sqlx::query("update provider_accounts set quota_limits = $2 where id = $1")
        .bind(id.as_str())
        .bind(sqlx::types::Json(limits(95)))
        .execute(&database.pool)
        .await
        .unwrap();
    let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    let later = start + Duration::from_secs(1);
    let merged = |base, at, paused| {
        let mut value = observation(id.as_str(), 1, at);
        value.quota_pause = Some(QuotaPauseDecision {
            limits: limits(95),
            paused,
            merge_base: Some(QuotaMergeBase { observed_at: base }),
        });
        value
    };
    assert_eq!(
        repository
            .compare_and_swap_quota(merged(None, start, true))
            .await
            .unwrap(),
        QuotaWriteOutcome::Updated
    );
    for (base, at) in [
        (None, later),
        (Some(start - Duration::from_secs(1)), later),
        // PostgreSQL 微秒精度下，同一时间不能代表两份不同快照
        (Some(start), start + Duration::from_nanos(1)),
    ] {
        assert_eq!(
            repository
                .compare_and_swap_quota(merged(base, at, false))
                .await
                .unwrap(),
            QuotaWriteOutcome::Conflict
        );
    }
    assert_eq!(
        repository
            .compare_and_swap_quota(observation(id.as_str(), 1, later))
            .await
            .unwrap(),
        QuotaWriteOutcome::Conflict
    );
    let mut wrong_policy = merged(Some(start), later, false);
    wrong_policy.quota_pause.as_mut().unwrap().limits = limits(90);
    assert_eq!(
        repository
            .compare_and_swap_quota(wrong_policy)
            .await
            .unwrap(),
        QuotaWriteOutcome::Conflict
    );
    assert!(
        repository
            .get_account(&id)
            .await
            .unwrap()
            .unwrap()
            .quota_paused()
    );

    assert_eq!(
        repository
            .touch_quota_observation(QuotaObservationTouch {
                account_id: id.clone(),
                expected_revision: CredentialRevision::new(1).unwrap(),
                observed_at: later,
            })
            .await
            .unwrap(),
        QuotaWriteOutcome::Updated
    );
    let persisted = repository
        .get_quotas(std::slice::from_ref(&id))
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(persisted.observed_at, later);
    assert_eq!(
        persisted.quota.expose_to_provider().get("marker"),
        Some(&json!("confirmed"))
    );
    assert!(
        repository
            .get_account(&id)
            .await
            .unwrap()
            .unwrap()
            .quota_paused()
    );
    assert_eq!(
        repository
            .compare_and_swap_quota(merged(Some(start), later + Duration::from_secs(1), false))
            .await
            .unwrap(),
        QuotaWriteOutcome::Conflict
    );
    assert_eq!(
        repository
            .compare_and_swap_quota(merged(Some(later), later + Duration::from_secs(1), false))
            .await
            .unwrap(),
        QuotaWriteOutcome::Updated
    );
    assert!(
        !repository
            .get_account(&id)
            .await
            .unwrap()
            .unwrap()
            .quota_paused()
    );
    database.close().await;
}

#[tokio::test]
async fn batch_quota_limits_roll_back_stale_preparations_and_preserve_omitted_policy() {
    let Some(database) = TestDatabase::create("quota_limits_batch_cas").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    let store = admin_account_store(&database.pool);
    let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    for id in ["acct_a", "acct_b"] {
        repository
            .insert_provider_account(account(id, id))
            .await
            .unwrap();
        repository
            .compare_and_swap_quota(observation(id, 1, start))
            .await
            .unwrap();
    }
    let context = MutationContext {
        actor: MutationActor::System,
        request_id: "quota-limit-config".to_owned(),
    };
    let command = batch(&["acct_a", "acct_b"], Some(limits(95)));
    let before = current_revision(&database.pool).await;
    assert!(
        store
            .batch_update_accounts(command.clone(), Default::default(), &context)
            .await
            .is_err()
    );
    let pauses: PreparedQuotaPauses = ["acct_a", "acct_b"]
        .into_iter()
        .map(|id| {
            (
                id.to_owned(),
                PreparedQuotaPause {
                    observed_at: Some(start.into()),
                    revision: gateway_admin::model::Revision::new(1).unwrap(),
                    paused: true,
                },
            )
        })
        .collect();
    for stale_revision in [true, false] {
        let mut stale = pauses.clone();
        let pause = stale.get_mut("acct_b").unwrap();
        if stale_revision {
            pause.revision = gateway_admin::model::Revision::new(2).unwrap();
        } else {
            pause.observed_at = Some((start - Duration::from_secs(1)).into());
        }
        let mut update = command.clone();
        update.enabled = Some(false);
        assert!(
            store
                .batch_update_accounts(update, stale, &context)
                .await
                .is_err()
        );
        assert_eq!(current_revision(&database.pool).await, before);
        assert_eq!(audit_count(&database.pool, &context.request_id).await, 0);
        for id in ["acct_a", "acct_b"] {
            let current = repository
                .load_provider_account(id)
                .await
                .unwrap()
                .unwrap()
                .summary;
            assert!(current.quota_limits.is_empty());
            assert!(!current.quota_paused);
            assert!(current.enabled);
            assert!(!current.allow_credits);
        }
    }
    let saved = store
        .batch_update_accounts(command.clone(), pauses, &context)
        .await
        .unwrap();
    assert_eq!(
        saved.config_revision.get(),
        u64::try_from(before + 1).unwrap()
    );
    assert_eq!(audit_count(&database.pool, &context.request_id).await, 1);
    let fields: Vec<String> = sqlx::query_scalar(
        "select changed_fields from admin_audit_events where admin_request_id = $1",
    )
    .bind(&context.request_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert!(fields.iter().any(|field| field == "quota_limits"));
    store
        .batch_update_accounts(batch(&["acct_a"], None), Default::default(), &context)
        .await
        .unwrap();
    let current = repository
        .load_provider_account("acct_a")
        .await
        .unwrap()
        .unwrap()
        .summary;
    assert_eq!(current.quota_limits, limits(95));
    assert!(current.quota_paused);
    store
        .batch_update_accounts(
            batch(&["acct_a"], Some(AccountQuotaLimits::default())),
            Default::default(),
            &context,
        )
        .await
        .unwrap();
    let current = repository
        .load_provider_account("acct_a")
        .await
        .unwrap()
        .unwrap()
        .summary;
    assert!(current.quota_limits.is_empty());
    assert!(!current.quota_paused);

    // 清空策略之后，在途的旧策略观测不能重新暂停账号
    let mut stale = observation("acct_a", 1, start + Duration::from_secs(1));
    stale.quota_pause = Some(QuotaPauseDecision {
        limits: limits(95),
        paused: true,
        merge_base: None,
    });
    assert_eq!(
        repository.compare_and_swap_quota(stale).await.unwrap(),
        QuotaWriteOutcome::Conflict
    );
    database.close().await;
}

#[tokio::test]
async fn proxy_changes_preserve_pause_evidence_and_reject_inflight_quota() {
    let Some(database) = TestDatabase::create("quota_proxy_pause").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    let proxies = PgProxyRepository::new(database.pool.clone());
    let admin = admin_account_store(&database.pool);
    let context = MutationContext {
        actor: MutationActor::System,
        request_id: "quota-proxy".to_owned(),
    };
    let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    for (index, change) in [
        "account",
        "url",
        "disabled",
        "unlink",
        "delete",
        "locked_disabled",
    ]
    .into_iter()
    .enumerate()
    {
        let id = ProviderAccountId::new(format!("acct_proxy_{index}")).unwrap();
        let mut seed = account(id.as_str(), id.as_str());
        seed.outbound_proxy = Some(
            gateway_core::account::OutboundProxy::parse(&format!(
                "http://127.0.0.1:{}",
                18080 + index
            ))
            .unwrap(),
        );
        repository.insert_provider_account(seed).await.unwrap();
        sqlx::query("update provider_accounts set quota_limits = $2 where id = $1")
            .bind(id.as_str())
            .bind(sqlx::types::Json(limits(95)))
            .execute(&database.pool)
            .await
            .unwrap();
        let current = repository.get_account(&id).await.unwrap().unwrap();
        let connection = UpstreamConnection::for_account(&current);
        let mut initial = observation(id.as_str(), 1, start);
        initial.expected_connection = Some(connection.clone());
        initial.quota_pause = Some(QuotaPauseDecision {
            limits: limits(95),
            paused: true,
            merge_base: None,
        });
        assert_eq!(
            repository.compare_and_swap_quota(initial).await.unwrap(),
            QuotaWriteOutcome::Updated
        );
        let mut stale = observation(id.as_str(), 1, start + Duration::from_secs(1));
        stale.expected_connection = Some(connection);
        stale.quota_pause = Some(QuotaPauseDecision {
            limits: limits(95),
            paused: false,
            merge_base: Some(QuotaMergeBase {
                observed_at: Some(start),
            }),
        });
        let proxy_id = repository
            .load_provider_account(id.as_str())
            .await
            .unwrap()
            .unwrap()
            .summary
            .outbound_proxy_id
            .unwrap();
        let proxy = proxies.get(&proxy_id).await.unwrap();
        match change {
            "account" => {
                let mut command = batch(&[id.as_str()], None);
                command.outbound_proxy = Some(AccountProxySelection::Direct);
                admin
                    .batch_update_accounts(command, Default::default(), &context)
                    .await
                    .unwrap();
            }
            "url" | "disabled" => {
                proxies
                    .update(
                        UpdateProxy {
                            id: proxy_id.clone(),
                            revision: proxy.revision,
                            name: proxy.name,
                            proxy: (change == "url").then(|| {
                                gateway_core::account::OutboundProxy::parse(
                                    "http://127.0.0.1:19090",
                                )
                                .unwrap()
                            }),
                            enabled: (change == "disabled").then_some(false),
                            max_websocket_connections: None,
                            auto_location: None,
                            location: None,
                            test: None,
                        },
                        &context,
                    )
                    .await
                    .unwrap();
            }
            "unlink" => {
                proxies
                    .remove_account(&proxy_id, &id, &context)
                    .await
                    .unwrap();
            }
            "delete" => {
                proxies
                    .delete(&proxy_id, proxy.revision, &context)
                    .await
                    .unwrap();
            }
            "locked_disabled" => {
                let mut transaction = database.pool.begin().await.unwrap();
                sqlx::query("select id from provider_accounts where id = $1 for update")
                    .bind(id.as_str())
                    .fetch_one(&mut *transaction)
                    .await
                    .unwrap();
                sqlx::query("update outbound_proxies set enabled = false where id = $1")
                    .bind(&proxy_id)
                    .execute(&mut *transaction)
                    .await
                    .unwrap();
                let writer = repository.clone();
                let mut pending =
                    tokio::spawn(async move { writer.compare_and_swap_quota(stale).await });
                assert!(
                    tokio::time::timeout(Duration::from_millis(20), &mut pending)
                        .await
                        .is_err()
                );
                transaction.commit().await.unwrap();
                assert_eq!(pending.await.unwrap().unwrap(), QuotaWriteOutcome::Conflict);
                stale = observation(id.as_str(), 1, start + Duration::from_secs(1));
                stale.expected_connection = Some(UpstreamConnection::for_account(&current));
            }
            _ => unreachable!(),
        }
        assert_eq!(
            repository.compare_and_swap_quota(stale).await.unwrap(),
            QuotaWriteOutcome::Conflict
        );
        let after = repository.get_account(&id).await.unwrap().unwrap();
        assert_eq!(after.revision(), current.revision());
        assert!(after.quota_paused());
        let snapshot = repository
            .get_quotas(std::slice::from_ref(&id))
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(snapshot.observed_at, start);
        assert_eq!(
            snapshot.quota.expose_to_provider().get("marker"),
            Some(&json!("confirmed"))
        );
    }
    database.close().await;
}

#[tokio::test]
async fn manual_recovery_preserves_local_limit_and_confirmed_pause_evidence() {
    let Some(database) = TestDatabase::create("quota_manual_recovery").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    let id = ProviderAccountId::new("acct_manual_pause").unwrap();
    repository
        .insert_provider_account(account(id.as_str(), "manual-pause"))
        .await
        .unwrap();
    let observed_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    let evidence = json!({"marker": "confirmed"});
    sqlx::query(
        "update provider_accounts set enabled = false, quota_limits = $2, quota_paused = true,
         provider_quota_json = $3, quota_observed_at = $4,
         updated_at = greatest(updated_at, $4) where id = $1",
    )
    .bind(id.as_str())
    .bind(sqlx::types::Json(limits(95)))
    .bind(&evidence)
    .bind(chrono::DateTime::<Utc>::from(observed_at))
    .execute(&database.pool)
    .await
    .unwrap();
    admin_account_store(&database.pool)
        .recover_account(
            &id,
            &MutationContext {
                actor: MutationActor::System,
                request_id: "quota-manual-recovery".to_owned(),
            },
        )
        .await
        .unwrap();
    let current = repository.get_account(&id).await.unwrap().unwrap();
    assert!(current.enabled());
    assert_eq!(current.quota().access(), QuotaAccessState::Allowed);
    assert!(current.quota_paused());
    assert_eq!(current.quota_limits(), &limits(95));
    let retained = repository.get_quotas(&[id]).await.unwrap().pop().unwrap();
    assert_eq!(retained.observed_at, observed_at);
    assert_eq!(
        retained.quota.expose_to_provider(),
        evidence.as_object().unwrap()
    );
    database.close().await;
}

#[tokio::test]
async fn credential_and_identity_revisions_preserve_confirmed_pause_snapshots() {
    let Some(database) = TestDatabase::create("quota_identity_pause").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    let id = ProviderAccountId::new("acct_identity_pause").unwrap();
    repository
        .insert_provider_account(account(id.as_str(), "original-user"))
        .await
        .unwrap();
    sqlx::query("update provider_accounts set quota_limits = $2 where id = $1")
        .bind(id.as_str())
        .bind(sqlx::types::Json(limits(95)))
        .execute(&database.pool)
        .await
        .unwrap();
    let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    let mut initial = observation(id.as_str(), 1, start);
    initial.quota_pause = Some(QuotaPauseDecision {
        limits: limits(95),
        paused: true,
        merge_base: None,
    });
    repository.compare_and_swap_quota(initial).await.unwrap();
    repository
        .compare_and_swap_credentials(credential_update(id.as_str(), 1, "manual-refresh"))
        .await
        .unwrap();
    let current = repository.get_account(&id).await.unwrap().unwrap();
    let refresh = CredentialCasUpdate::new(
        id.clone(),
        current.revision(),
        ProviderAccountUpdate {
            account_id: id.clone(),
            name: "new profile".to_owned(),
            email: None,
            plan_type: None,
        },
        plaintext_credential("background-refresh"),
        false,
        current.access_token_expires_at(),
        None,
    )
    .unwrap();
    assert!(matches!(
        repository
            .compare_and_swap_credential(refresh)
            .await
            .unwrap(),
        CredentialCasOutcome::Updated(_)
    ));
    repository
        .rotate_provider_account(RotateProviderAccount {
            settings: None,
            scope: ProviderAccountAdminScope {
                provider_kind: "openai".to_owned(),
            },
            profile: profile(id.as_str(), "new identity"),
            replacement_identity: Some(ProviderAccountIdentity::new(
                "replacement-user".to_owned(),
                None,
            )),
            credential: credential_update(id.as_str(), 3, "replacement-credential"),
            audit: audit("quota_identity_rotate", "rotate", id.as_str()),
        })
        .await
        .unwrap();
    repository
        .import_provider_accounts(ImportProviderAccounts {
            scope: ProviderAccountAdminScope {
                provider_kind: "openai".to_owned(),
            },
            accounts: vec![account("unused-import-id", "replacement-user")],
            audit: audit("quota_identity_import", "import", id.as_str()),
            settings: None,
            outbound_proxy: None,
        })
        .await
        .unwrap();
    let current = repository.get_account(&id).await.unwrap().unwrap();
    assert_eq!(current.revision().get(), 5);
    assert!(current.quota_paused());
    assert_eq!(current.quota_limits(), &limits(95));
    let snapshot = repository
        .get_quotas(std::slice::from_ref(&id))
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(snapshot.observed_at, start);
    assert_eq!(
        snapshot.quota.expose_to_provider().get("marker"),
        Some(&json!("confirmed"))
    );
    assert_eq!(
        repository
            .compare_and_swap_quota(observation(id.as_str(), 1, start + Duration::from_secs(1)))
            .await
            .unwrap(),
        QuotaWriteOutcome::Conflict
    );
    database.close().await;
}
