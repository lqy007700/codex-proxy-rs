//! 验证真实 PostgreSQL 亲和日志的幂等、并发终态、事务回滚及保留期

use chrono::{Duration, Utc};
use gateway_admin::{
    model::{MutationActor, MutationContext, affinity::*},
    ports::affinity::AffinityJournalStore as _,
};
use gateway_core::{
    policy::CallerIdentity,
    provider_ports::{ProviderSessionObservation, SessionBindingDirective},
};
use gateway_store::postgres::PgAffinityJournal;

fn context() -> MutationContext {
    MutationContext {
        actor: MutationActor::System,
        request_id: "affinity-fixture".into(),
    }
}
fn plan() -> AffinityPlan {
    let now = Utc::now();
    AffinityPlan {
        operation_id: uuid::Uuid::new_v4().to_string(),
        directive: SessionBindingDirective::Clear,
        created_at: now,
        expires_at: now + Duration::minutes(15),
        items: vec![AffinityPlanItem {
            session: AffinitySession {
                reference: "synthetic-session".into(),
                provider: "openai".into(),
                observation: ProviderSessionObservation {
                    binding_scope: Default::default(),
                    caller: CallerIdentity::Direct {
                        key_id: "key_fixture".into(),
                    },
                    client_key_id: "key_fixture".into(),
                    request_id: "req_fixture".into(),
                    actual_account_id: "acct_a".into(),
                },
                account_id: "acct_a".into(),
                revision: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                directive: None,
                first_observed_at: now,
                last_admission_at: now,
                expires_at: now + Duration::hours(1),
            },
            next_revision: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
            rejection: None,
        }],
    }
}
fn result(status: &str) -> AffinityItemResult {
    AffinityItemResult {
        provider: "openai".into(),
        reference: "synthetic-session".into(),
        status: status.into(),
        message: status.into(),
        audit_recorded: false,
    }
}

#[tokio::test]
async fn journal_replays_plan_rejects_id_collision_and_commits_terminal_result_with_one_audit() {
    let Some(database) = super::TestDatabase::create("affinity_journal").await else {
        return;
    };
    let journal = PgAffinityJournal::new(database.pool.clone());
    let plan = plan();
    journal.create_plan(&context(), plan.clone()).await.unwrap();
    journal.create_plan(&context(), plan.clone()).await.unwrap();
    let mut collision = plan.clone();
    collision.directive = SessionBindingDirective::Reassign;
    assert!(journal.create_plan(&context(), collision).await.is_err());
    let count: i64 = sqlx::query_scalar("select count(*) from admin_audit_events")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let applied = result("applied");
    let failed = result("failed");
    let actor = context();
    let (one, two) = tokio::join!(
        journal.record_result(&actor, &plan.operation_id, &applied),
        journal.record_result(&actor, &plan.operation_id, &failed)
    );
    assert!(one.unwrap().audit_recorded);
    two.unwrap();
    assert_eq!(
        journal
            .operation(&plan.operation_id)
            .await
            .unwrap()
            .unwrap()
            .results["openai:synthetic-session"]
            .status,
        "applied"
    );
    let before: i64 = sqlx::query_scalar("select count(*) from admin_audit_events")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    let replay = journal
        .record_result(&context(), &plan.operation_id, &result("rejected"))
        .await
        .unwrap();
    assert_eq!(replay.status, "applied");
    let after: i64 = sqlx::query_scalar("select count(*) from admin_audit_events")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(before, after);
    database.close().await;
}

#[tokio::test]
async fn journal_rolls_back_result_on_audit_failure_and_reaps_only_expired_operation_plans() {
    let Some(database) = super::TestDatabase::create("affinity_audit_rollback").await else {
        return;
    };
    let journal = PgAffinityJournal::new(database.pool.clone());
    let active = plan();
    journal
        .create_plan(&context(), active.clone())
        .await
        .unwrap();
    // 真实外键失败证明日志与结果在同一事务内提交
    let invalid_actor = MutationContext {
        actor: MutationActor::AdminSession {
            admin_user_id: "missing-admin".into(),
        },
        request_id: "invalid-audit".into(),
    };
    assert!(
        journal
            .record_result(&invalid_actor, &active.operation_id, &result("applied"))
            .await
            .is_err()
    );
    assert!(
        journal
            .operation(&active.operation_id)
            .await
            .unwrap()
            .unwrap()
            .results
            .is_empty()
    );
    let mut stale = plan();
    stale.expires_at = Utc::now() - Duration::days(31);
    journal
        .create_plan(&context(), stale.clone())
        .await
        .unwrap();
    journal.create_plan(&context(), plan()).await.unwrap();
    assert!(
        journal
            .operation(&stale.operation_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        journal
            .operation(&active.operation_id)
            .await
            .unwrap()
            .is_some()
    );
    database.close().await;
}

#[tokio::test]
async fn target_validation_obeys_key_group_source_and_account_state() {
    let Some(database) = super::TestDatabase::create("affinity_eligibility").await else {
        return;
    };
    let journal = PgAffinityJournal::new(database.pool.clone());
    sqlx::query("insert into client_api_keys(id,name,key,created_at,updated_at) values('key_fixture','fixture','synthetic-affinity-key',now(),now())").execute(&database.pool).await.unwrap();
    for id in ["acct_a", "acct_b"] {
        sqlx::query("insert into provider_accounts(id,provider_kind,name,authentication_kind,provider_credentials_json,has_refresh_token,credential_observed_at,created_at,updated_at) values($1,'openai',$1,'oauth','{}',false,now(),now(),now())").bind(id).execute(&database.pool).await.unwrap();
    }
    let session = plan().items.remove(0).session;
    assert!(
        journal
            .eligible_target(&session, Some("acct_b"))
            .await
            .unwrap()
    );
    assert!(journal.eligible_target(&session, None).await.unwrap());
    for sql in [
        "update provider_accounts set enabled=false where id='acct_b'",
        "update provider_accounts set enabled=true,credential_state='invalid' where id='acct_b'",
        "update provider_accounts set credential_state='ready',scheduling_role='fallback' where id='acct_b'",
    ] {
        sqlx::query(sql).execute(&database.pool).await.unwrap();
        assert!(
            !journal
                .eligible_target(&session, Some("acct_b"))
                .await
                .unwrap()
        );
    }
    sqlx::query("update provider_accounts set scheduling_role='regular' where id='acct_b'")
        .execute(&database.pool)
        .await
        .unwrap();
    sqlx::query("insert into account_groups(id,name,enabled,created_at,updated_at,color) values('grp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','scope',true,now(),now(),'#123456FF')").execute(&database.pool).await.unwrap();
    sqlx::query("insert into account_group_accounts(account_group_id,provider_account_id,created_at) values('grp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','acct_a',now())").execute(&database.pool).await.unwrap();
    sqlx::query("insert into client_api_key_groups(client_api_key_id,account_group_id,created_at) values('key_fixture','grp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',now())").execute(&database.pool).await.unwrap();
    assert!(
        !journal
            .eligible_target(&session, Some("acct_b"))
            .await
            .unwrap()
    );
    assert!(
        journal
            .eligible_target(&session, Some("acct_a"))
            .await
            .unwrap()
    );
    let source = IdentitySource {
        id: uuid::Uuid::new_v4().to_string(),
        name: "fixture".into(),
        enabled: true,
    };
    journal
        .save_source(&context(), source.clone())
        .await
        .unwrap();
    let mut proxy_session = session;
    proxy_session.observation.caller = CallerIdentity::NewApi {
        source_id: source.id.clone(),
        user_id: "1".into(),
        token_id: "11".into(),
    };
    sqlx::query("update client_api_keys set identity_policy_json=$1 where id='key_fixture'")
        .bind(serde_json::json!({"mode":"newApi","sourceId":source.id,"allowUnidentified":false}))
        .execute(&database.pool)
        .await
        .unwrap();
    assert!(
        journal
            .eligible_target(&proxy_session, Some("acct_a"))
            .await
            .unwrap()
    );
    journal
        .save_source(
            &context(),
            IdentitySource {
                enabled: false,
                ..source
            },
        )
        .await
        .unwrap();
    assert!(
        !journal
            .eligible_target(&proxy_session, Some("acct_a"))
            .await
            .unwrap()
    );
    database.close().await;
}
