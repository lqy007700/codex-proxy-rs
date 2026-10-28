//! 额度预警候选隔离、十分钟门禁、周期去重和 outbox 原子性测试

use super::*;
use gateway_admin::alerts::quota::evaluate as evaluate_quota;
use gateway_store::postgres::{PgProviderAccountRepository, ProviderAccountRepository as _};

async fn quota_settings(repository: &PgAlertRepository) -> AlertSettings {
    let settings = configure(repository).await;
    let mut values = settings.values;
    values.enabled = false;
    values.quota_warning = Some(QuotaWarningSettings {
        enabled: true,
        ..QuotaWarningSettings::default()
    });
    repository
        .update_settings(
            UpdateAlertSettings {
                expected_revision: settings.revision,
                values,
                password: None,
                clear_password: false,
            },
            &context(),
        )
        .await
        .unwrap()
}

async fn seed(db: &TestDatabase) {
    let accounts = PgProviderAccountRepository::new(db.pool.clone());
    for id in ["acct_a", "acct_b", "acct_disabled", "acct_api"] {
        let mut value = super::super::provider_accounts::account(id, id);
        if id == "acct_disabled" {
            value.enabled = false;
        }
        if id == "acct_api" {
            value.authentication_kind = "api_key".to_owned();
        }
        accounts.insert_provider_account(value).await.unwrap();
    }
    sqlx::query("update provider_accounts set scheduling_role='fallback' where id='acct_b'")
        .execute(&db.pool)
        .await
        .unwrap();
}

async fn evaluation(
    repository: &PgAlertRepository,
    settings: &AlertSettings,
    now: DateTime<Utc>,
) -> QuotaWarningEvaluation {
    let accounts = repository
        .quota_accounts(None)
        .await
        .unwrap()
        .into_iter()
        .map(|mut account| {
            account.observed_at = Some(now);
            (
                account,
                Some(vec![QuotaWarningWindow {
                    key: "codex:18000s".to_owned(),
                    period: QuotaWarningPeriod::ShortTerm,
                    used_percent: Some(90.0),
                    reset_at: Some(now + TimeDelta::hours(5)),
                }]),
            )
        })
        .collect();
    evaluate_quota(
        &settings.values.quota_warning(),
        accounts,
        &repository.quota_states().await.unwrap(),
        now,
    )
}

#[tokio::test]
async fn quota_digest_is_independent_of_failure_alerts_and_fenced_for_ten_minutes_after_restart() {
    let Some(db) = TestDatabase::create("quota_digest").await else {
        return;
    };
    seed(&db).await;
    let repository = PgAlertRepository::new(db.pool.clone(), observability_query_budget());
    let settings = quota_settings(&repository).await;
    let now = Utc::now();
    let result = evaluation(&repository, &settings, now).await;
    assert_eq!(result.items.len(), 2);
    assert_eq!(result.status.accounts, 2);
    assert!(
        repository
            .commit_quota_evaluation(settings.revision, None, result.clone())
            .await
            .unwrap()
    );
    assert!(
        !repository
            .commit_quota_evaluation(settings.revision, None, result)
            .await
            .unwrap()
    );
    let restarted = PgAlertRepository::new(db.pool.clone(), observability_query_budget());
    let status = restarted.quota_status().await.unwrap();
    assert_eq!(status.checked_at, Some(now));
    assert!(
        restarted
            .quota_accounts(Some("acct_b".to_owned()))
            .await
            .unwrap()
            .is_empty()
    );
    let premature = evaluation(&restarted, &settings, now + TimeDelta::minutes(9)).await;
    assert!(
        !restarted
            .commit_quota_evaluation(settings.revision, Some(now), premature)
            .await
            .unwrap()
    );
    let later = evaluation(&restarted, &settings, now + TimeDelta::minutes(10)).await;
    assert!(later.items.is_empty());
    assert!(
        restarted
            .commit_quota_evaluation(settings.revision, Some(now), later)
            .await
            .unwrap()
    );
    let deliveries = restarted.status().await.unwrap().deliveries;
    assert_eq!(deliveries.len(), 2);
    assert!(
        deliveries
            .iter()
            .all(|record| record.kind == AlertNotificationKind::QuotaWarning)
    );
    let first = restarted.claim_delivery().await.unwrap().unwrap();
    assert!(first.subject.contains("2 个账号、2 个额度窗口"));
    restarted
        .complete_delivery(&first.id, first.attempt, true)
        .await
        .unwrap();
    let second = restarted.claim_delivery().await.unwrap().unwrap();
    assert_ne!(first.recipient, second.recipient);
    restarted
        .complete_delivery(&second.id, second.attempt, false)
        .await
        .unwrap();
    assert!(restarted.claim_delivery().await.unwrap().is_none());
    assert_eq!(restarted.quota_states().await.unwrap().len(), 2);
    db.close().await;
}

#[tokio::test]
async fn full_outbox_rolls_back_quota_reservations_and_check_progress() {
    let Some(db) = TestDatabase::create("quota_full_queue").await else {
        return;
    };
    seed(&db).await;
    let repository = PgAlertRepository::new(db.pool.clone(), observability_query_budget());
    let settings = quota_settings(&repository).await;
    sqlx::query("insert into alert_notifications(id,kind,settings_revision,notification_key,recipient,subject,body) select 'full-'||n,'test',$1,'full-'||n,'example@example.test','test','test' from generate_series(1,100) n")
        .bind(i64::try_from(settings.revision).unwrap()).execute(&db.pool).await.unwrap();
    let result = evaluation(&repository, &settings, Utc::now()).await;
    assert!(
        repository
            .commit_quota_evaluation(settings.revision, None, result)
            .await
            .is_err()
    );
    assert!(repository.quota_states().await.unwrap().is_empty());
    assert!(
        repository
            .quota_status()
            .await
            .unwrap()
            .checked_at
            .is_none()
    );
    db.close().await;
}

#[tokio::test]
async fn quota_only_configuration_changes_preserve_failure_incidents_and_cycle_reservations() {
    let Some(db) = TestDatabase::create("quota_configuration").await else {
        return;
    };
    seed(&db).await;
    let repository = PgAlertRepository::new(db.pool.clone(), observability_query_budget());
    let settings = configure(&repository).await;
    let now = Utc::now();
    super::evaluate_at(&repository, &settings, now, 30).await;
    let mut values = settings.values;
    values.quota_warning = Some(QuotaWarningSettings {
        enabled: true,
        ..QuotaWarningSettings::default()
    });
    let settings = repository
        .update_settings(
            UpdateAlertSettings {
                expected_revision: settings.revision,
                values,
                password: None,
                clear_password: false,
            },
            &context(),
        )
        .await
        .unwrap();
    assert_eq!(
        repository.status().await.unwrap().events[0].status,
        "firing"
    );
    assert!(
        repository
            .status()
            .await
            .unwrap()
            .deliveries
            .iter()
            .all(|record| record.status == "pending")
    );
    let result = evaluation(&repository, &settings, now).await;
    repository
        .commit_quota_evaluation(settings.revision, None, result)
        .await
        .unwrap();
    let mut values = settings.values;
    values
        .quota_warning
        .as_mut()
        .unwrap()
        .short_term_threshold_percent = 85;
    let settings = repository
        .update_settings(
            UpdateAlertSettings {
                expected_revision: settings.revision,
                values,
                password: None,
                clear_password: false,
            },
            &context(),
        )
        .await
        .unwrap();
    let again = evaluation(&repository, &settings, now + TimeDelta::minutes(10)).await;
    assert!(again.items.is_empty());
    assert_eq!(
        repository.status().await.unwrap().events[0].status,
        "firing"
    );
    db.close().await;
}

#[tokio::test]
async fn competing_instances_cannot_enqueue_the_same_quota_cycle_twice() {
    let Some(db) = TestDatabase::create("quota_competing").await else {
        return;
    };
    seed(&db).await;
    let first = PgAlertRepository::new(db.pool.clone(), observability_query_budget());
    let second = PgAlertRepository::new(db.pool.clone(), observability_query_budget());
    let settings = quota_settings(&first).await;
    let result = evaluation(&first, &settings, Utc::now()).await;
    let (left, right) = tokio::join!(
        first.commit_quota_evaluation(settings.revision, None, result.clone()),
        second.commit_quota_evaluation(settings.revision, None, result),
    );
    assert_eq!(
        usize::from(matches!(left, Ok(true))) + usize::from(matches!(right, Ok(true))),
        1
    );
    second.quota_failed(settings.revision, None).await.unwrap();
    assert!(!first.quota_status().await.unwrap().error);
    let checkpoint = first.quota_status().await.unwrap().checked_at;
    second
        .quota_failed(settings.revision, checkpoint)
        .await
        .unwrap();
    assert!(first.quota_status().await.unwrap().error);
    assert_eq!(first.status().await.unwrap().deliveries.len(), 2);
    assert_eq!(first.quota_states().await.unwrap().len(), 2);
    db.close().await;
}
