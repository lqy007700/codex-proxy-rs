//! 验证 OAuth 换号与重置卡计划的原子隔离、未决保护和旧任务 CAS

use gateway_admin::{
    model::reset_credit_automation::{
        PendingResetCredit, PlannedResetCredit, ResetCreditAutomation, ResetCreditAutomationState,
    },
    ports::reset_credit_automation::ResetCreditAutomationStore,
};

use super::*;

fn plan(id: &str, pending: bool) -> ResetCreditAutomation {
    let now = Utc::now();
    let credit = PlannedResetCredit {
        id: "old-identity-credit".into(),
        expires_at: now + TimeDelta::hours(1),
    };
    ResetCreditAutomation {
        account_id: id.into(),
        enabled: true,
        revision: 0,
        next_run_at: now + TimeDelta::minutes(20),
        state: ResetCreditAutomationState {
            next_discovery_at: now + TimeDelta::hours(6),
            credits: vec![credit.clone()],
            pending: pending.then(|| PendingResetCredit {
                credit,
                redeem_request_id: uuid::Uuid::new_v4().to_string(),
            }),
            retry_at: Some(now + TimeDelta::minutes(1)),
            failures: 2,
            last_result: Some("retry".into()),
        },
    }
}

fn rotation(id: &str, revision: u64, replace: bool) -> RotateProviderAccount {
    RotateProviderAccount {
        settings: None,
        scope: ProviderAccountAdminScope {
            provider_kind: "openai".into(),
        },
        profile: profile(id, "local account"),
        replacement_identity: replace.then(|| {
            ProviderAccountIdentity::new(
                "replacement-user".into(),
                Some("replacement-workspace".into()),
            )
        }),
        credential: credential_update(id, revision, "replacement-test-only"),
        audit: audit(&format!("audit_{id}_{revision}"), "reauthorize", id),
    }
}

#[tokio::test]
async fn replacing_identity_resets_plan_and_rejects_old_worker_results() {
    let Some(database) = TestDatabase::create("reauth_reset_plan").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    let store = admin_account_store(&database.pool);
    let id = "acct_reset_plan";
    repository
        .insert_provider_account(account(id, "original-user"))
        .await
        .unwrap();
    let original = plan(id, false);
    store
        .save_reset_credit_automation(&original, None)
        .await
        .unwrap();
    let original = store.reset_credit_automation(id).await.unwrap().unwrap();
    repository
        .rotate_provider_account(rotation(id, 1, true))
        .await
        .unwrap();
    let current = store.reset_credit_automation(id).await.unwrap().unwrap();
    assert!(current.enabled);
    assert_eq!(current.revision, original.revision + 1);
    assert!(current.state.credits.is_empty());
    assert!(current.state.pending.is_none());
    assert!(current.state.retry_at.is_none());
    assert_eq!(current.state.failures, 0);
    assert!(current.state.last_result.is_none());
    assert!(current.next_run_at <= Utc::now());
    assert!(
        store
            .save_reset_credit_automation(&original, None)
            .await
            .is_err()
    );
    database.close().await;
}

#[tokio::test]
async fn pending_consumption_blocks_identity_change_but_allows_same_identity_refresh() {
    let Some(database) = TestDatabase::create("reauth_pending_credit").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    let store = admin_account_store(&database.pool);
    let id = "acct_pending_credit";
    repository
        .insert_provider_account(account(id, "original-user"))
        .await
        .unwrap();
    let original = plan(id, true);
    store
        .save_reset_credit_automation(&original, None)
        .await
        .unwrap();
    let original = store.reset_credit_automation(id).await.unwrap().unwrap();
    let before = current_revision(&database.pool).await;
    assert!(matches!(
        repository
            .rotate_provider_account(rotation(id, 1, true))
            .await,
        Err(StoreError::Conflict {
            kind: ConflictKind::InvalidTransition,
            ..
        })
    ));
    assert_eq!(current_revision(&database.pool).await, before);
    let current = repository
        .get_account(&ProviderAccountId::new(id).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.revision().get(), 1);
    assert_eq!(current.upstream_user_id(), Some("original-user"));
    assert_eq!(
        store.reset_credit_automation(id).await.unwrap(),
        Some(original.clone())
    );
    repository
        .rotate_provider_account(rotation(id, 1, false))
        .await
        .unwrap();
    assert_eq!(
        store.reset_credit_automation(id).await.unwrap(),
        Some(original)
    );
    database.close().await;
}

#[tokio::test]
async fn duplicate_identity_rolls_back_plan_reset_and_credential_rotation() {
    let Some(database) = TestDatabase::create("reauth_duplicate_credit").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    let store = admin_account_store(&database.pool);
    let id = "acct_duplicate_plan";
    repository
        .insert_provider_account(account(id, "original-user"))
        .await
        .unwrap();
    let mut duplicate = account("acct_existing_identity", "replacement-user");
    duplicate.upstream_account_id = Some("replacement-workspace".into());
    repository.insert_provider_account(duplicate).await.unwrap();
    let original = plan(id, false);
    store
        .save_reset_credit_automation(&original, None)
        .await
        .unwrap();
    let original = store.reset_credit_automation(id).await.unwrap().unwrap();
    assert!(
        repository
            .rotate_provider_account(rotation(id, 1, true))
            .await
            .is_err()
    );
    assert_eq!(
        store.reset_credit_automation(id).await.unwrap(),
        Some(original)
    );
    let current = repository
        .get_account(&ProviderAccountId::new(id).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.revision().get(), 1);
    assert_eq!(current.upstream_user_id(), Some("original-user"));
    database.close().await;
}
