//! 告警独立版本、完成时间采样、事件原子性和收件人级 outbox 恢复测试

mod quota;

use chrono::{DateTime, TimeDelta, Utc};
use gateway_admin::{
    alerts::policy::{evaluate, notification_text},
    model::{MutationActor, MutationContext, alerts::*},
    ports::{alerts::AlertRepository as _, store::AdminStoreErrorKind},
};
use gateway_core::engine::{ExecutionOutcome, ExecutionStore as _};
use gateway_store::postgres::{PgAlertRepository, PgExecutionStore};

use super::{TestDatabase, observability_query_budget};

fn context() -> MutationContext {
    MutationContext {
        actor: MutationActor::System,
        request_id: "alerts-integration".to_owned(),
    }
}

async fn configure(repository: &PgAlertRepository) -> AlertSettings {
    let settings = repository.settings().await.unwrap();
    repository
        .update_settings(
            UpdateAlertSettings {
                expected_revision: settings.revision,
                values: AlertSettingsValues {
                    enabled: true,
                    consecutive_evaluations: 1,
                    smtp_host: "localhost".to_owned(),
                    sender_address: "alerts@example.test".to_owned(),
                    recipients: vec![
                        "first@example.test".to_owned(),
                        "second@example.test".to_owned(),
                    ],
                    ..AlertSettingsValues::default()
                },
                password: Some(secrecy::SecretString::from("FAKE_SMTP_PASSWORD")),
                clear_password: false,
            },
            &context(),
        )
        .await
        .unwrap()
}

async fn evaluate_at(
    repository: &PgAlertRepository,
    settings: &AlertSettings,
    now: DateTime<Utc>,
    failures: u64,
) {
    let previous = repository.runtime().await.unwrap();
    let evaluation = evaluate(
        &settings.values,
        &previous,
        Some(AlertSample {
            window_start: now - TimeDelta::minutes(5),
            window_end: now,
            requests: 100,
            failures,
        }),
        now,
        format!("incident-{}", previous.generation),
    );
    let (subject, body) = notification_text(&settings.values, &evaluation);
    assert!(
        repository
            .commit_evaluation(
                settings.revision,
                previous.generation,
                evaluation,
                subject,
                body,
            )
            .await
            .unwrap()
    );
}

async fn recover(
    repository: &PgAlertRepository,
    settings: &AlertSettings,
    previous: DateTime<Utc>,
) {
    evaluate_at(repository, settings, previous + TimeDelta::minutes(1), 0).await;
    evaluate_at(repository, settings, previous + TimeDelta::minutes(2), 0).await;
    assert_eq!(
        repository.status().await.unwrap().events[0].status,
        "recovered"
    );
}

async fn delivery_record(repository: &PgAlertRepository, id: &str) -> AlertDeliveryRecord {
    repository
        .status()
        .await
        .unwrap()
        .deliveries
        .into_iter()
        .find(|record| record.id == id)
        .unwrap()
}

#[tokio::test]
async fn settings_use_their_own_revision_and_never_audit_password_contents() {
    let Some(db) = TestDatabase::create("alerts_settings").await else {
        return;
    };
    let repository = PgAlertRepository::new(db.pool.clone(), observability_query_budget());
    let before: i64 = sqlx::query_scalar("select config_revision from runtime_settings where id=1")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let settings = configure(&repository).await;
    assert_eq!(settings.revision, 2);
    assert_eq!(
        repository.runtime().await.unwrap().evaluation_status,
        AlertEvaluationStatus::Waiting
    );
    let stale = repository
        .update_settings(
            UpdateAlertSettings {
                expected_revision: 1,
                values: settings.values.clone(),
                password: None,
                clear_password: false,
            },
            &context(),
        )
        .await
        .unwrap_err();
    assert_eq!(stale.kind(), AdminStoreErrorKind::StaleRevision);
    let after: i64 = sqlx::query_scalar("select config_revision from runtime_settings where id=1")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(before, after);
    let audit: String =
        sqlx::query_scalar("select string_agg(row_to_json(a)::text,' ') from admin_audit_events a")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(audit.contains("alerts.settings_updated"));
    assert!(audit.contains("smtpPassword"));
    assert!(!audit.contains("FAKE_SMTP_PASSWORD"));
    let unchanged = repository
        .update_settings(
            UpdateAlertSettings {
                expected_revision: settings.revision,
                values: settings.values,
                password: None,
                clear_password: false,
            },
            &context(),
        )
        .await
        .unwrap();
    assert_eq!(unchanged.revision, 2);
    db.close().await;
}

#[tokio::test]
async fn sampling_uses_completion_time_and_excludes_cancelled_prewarm_and_internal_requests() {
    let Some(db) = TestDatabase::create("alerts_sampling").await else {
        return;
    };
    let repository = PgAlertRepository::new(db.pool.clone(), observability_query_budget());
    let execution = PgExecutionStore::new(db.pool.clone());
    let now = Utc::now();
    for (id, outcome, kind, transport, completed_offset) in [
        ("failure", ExecutionOutcome::Failed, None, "http_sse", -10),
        (
            "incomplete",
            ExecutionOutcome::Incomplete,
            None,
            "http_sse",
            -10,
        ),
        (
            "success_after_retries",
            ExecutionOutcome::Succeeded,
            None,
            "http_sse",
            -10,
        ),
        (
            "cancelled",
            ExecutionOutcome::Cancelled,
            None,
            "http_sse",
            -10,
        ),
        (
            "warmup",
            ExecutionOutcome::Failed,
            Some("prewarm"),
            "http_sse",
            -10,
        ),
        ("internal", ExecutionOutcome::Failed, None, "internal", -10),
        (
            "plugin",
            ExecutionOutcome::Failed,
            None,
            "internal_plugin",
            -10,
        ),
        ("outside", ExecutionOutcome::Failed, None, "http_sse", -600),
        ("at_end", ExecutionOutcome::Failed, None, "http_sse", 0),
    ] {
        let mut request = super::execution::accepted_request(&format!("req_{id}"));
        request.started_at = (now - TimeDelta::minutes(20)).into();
        request.deadline_at = std::time::SystemTime::from(now + TimeDelta::minutes(1)).into();
        request.request_kind = kind.map(str::to_owned);
        request.client_transport = transport.to_owned();
        execution
            .create_model_request(request.clone())
            .await
            .unwrap();
        let mut finalization = super::execution::early_failure(&request);
        finalization.outcome = outcome;
        finalization.completed_at = (now + TimeDelta::seconds(completed_offset)).into();
        if matches!(outcome, ExecutionOutcome::Succeeded) {
            finalization.error = None;
            finalization.client_status_code = Some(200);
            finalization.downstream_committed_at = Some(request.started_at);
        }
        execution
            .finalize_model_request(finalization)
            .await
            .unwrap();
    }
    let sample = repository
        .sample(now - TimeDelta::minutes(5), now)
        .await
        .unwrap();
    assert_eq!(sample.requests, 3);
    assert_eq!(sample.failures, 2);
    db.close().await;
}

#[tokio::test]
async fn evaluation_and_outbox_commit_once_and_stale_evaluations_cannot_overwrite_changes() {
    let Some(db) = TestDatabase::create("alerts_evaluation").await else {
        return;
    };
    let repository = PgAlertRepository::new(db.pool.clone(), observability_query_budget());
    let settings = configure(&repository).await;
    let previous = repository.runtime().await.unwrap();
    let now = Utc::now();
    let evaluation = evaluate(
        &settings.values,
        &previous,
        Some(AlertSample {
            window_start: now - TimeDelta::minutes(5),
            window_end: now,
            requests: 100,
            failures: 30,
        }),
        now,
        "incident-1".to_owned(),
    );
    assert!(
        repository
            .commit_evaluation(
                settings.revision,
                previous.generation,
                evaluation.clone(),
                "告警".to_owned(),
                "统计摘要".to_owned()
            )
            .await
            .unwrap()
    );
    assert!(
        !repository
            .commit_evaluation(
                settings.revision,
                previous.generation,
                evaluation.clone(),
                "告警".to_owned(),
                "统计摘要".to_owned()
            )
            .await
            .unwrap()
    );
    let status = repository.status().await.unwrap();
    assert_eq!(status.events.len(), 1);
    assert_eq!(status.deliveries.len(), 2);
    let mut values = settings.values;
    values.enabled = false;
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
        .unwrap();
    assert!(
        !repository
            .commit_evaluation(
                settings.revision,
                previous.generation,
                evaluation,
                String::new(),
                String::new()
            )
            .await
            .unwrap()
    );
    assert!(repository.claim_delivery().await.unwrap().is_none());
    let status = repository.status().await.unwrap();
    assert_eq!(status.events[0].status, "cancelled");
    assert!(
        status
            .deliveries
            .iter()
            .all(|record| record.status == "cancelled")
    );
    db.close().await;
}

#[tokio::test]
async fn smtp_delivery_releases_connections_and_retries_only_the_failed_recipient() {
    let Some(db) = TestDatabase::create("alerts_outbox").await else {
        return;
    };
    let repository = PgAlertRepository::new(db.pool.clone(), observability_query_budget());
    let settings = configure(&repository).await;
    repository
        .queue_test(settings.revision, &context())
        .await
        .unwrap();
    assert!(
        repository
            .queue_test(settings.revision, &context())
            .await
            .is_err()
    );
    let first = repository.claim_delivery().await.unwrap().unwrap();
    repository
        .complete_delivery(&first.id, first.attempt, true)
        .await
        .unwrap();
    let second = repository.claim_delivery().await.unwrap().unwrap();
    assert_ne!(first.recipient, second.recipient);
    // 已领取消息不持有连接，仍能在同一连接预算中读取设置和执行其他 SQL
    assert!(repository.settings().await.is_ok());
    repository
        .complete_delivery(&second.id, second.attempt, false)
        .await
        .unwrap();
    assert!(repository.claim_delivery().await.unwrap().is_none());
    sqlx::query(
        "update alert_notifications set next_attempt_at=now()-interval '1 second' where id=$1",
    )
    .bind(&second.id)
    .execute(&db.pool)
    .await
    .unwrap();
    let retry = repository.claim_delivery().await.unwrap().unwrap();
    assert_eq!(retry.id, second.id);
    assert_eq!(retry.attempt, 2);
    // 模拟发送中进程退出，旧 claim 到期后恢复，迟到的旧结果不能完成新尝试
    sqlx::query(
        "update alert_notifications set claimed_until=now()-interval '1 second' where id=$1",
    )
    .bind(&retry.id)
    .execute(&db.pool)
    .await
    .unwrap();
    let restored = repository.claim_delivery().await.unwrap().unwrap();
    assert_eq!(restored.attempt, 3);
    repository
        .complete_delivery(&retry.id, retry.attempt, true)
        .await
        .unwrap();
    repository
        .complete_delivery(&restored.id, restored.attempt, false)
        .await
        .unwrap();
    let records = repository.status().await.unwrap().deliveries;
    assert_eq!(
        records
            .iter()
            .find(|record| record.id == first.id)
            .unwrap()
            .status,
        "sent"
    );
    assert_eq!(
        records
            .iter()
            .find(|record| record.id == second.id)
            .unwrap()
            .status,
        "failed"
    );
    assert!(repository.claim_delivery().await.unwrap().is_none());
    db.close().await;
}

#[tokio::test]
async fn recovery_preserves_inflight_results_and_cancels_failed_old_alerts() {
    let Some(db) = TestDatabase::create("alerts_recovery_failure").await else {
        return;
    };
    let repository = PgAlertRepository::new(db.pool.clone(), observability_query_budget());
    let settings = configure(&repository).await;
    let now = Utc::now();
    evaluate_at(&repository, &settings, now, 30).await;
    let delivered = repository.claim_delivery().await.unwrap().unwrap();
    repository
        .complete_delivery(&delivered.id, delivered.attempt, true)
        .await
        .unwrap();
    let inflight = repository.claim_delivery().await.unwrap().unwrap();
    recover(&repository, &settings, now).await;
    assert_eq!(
        delivery_record(&repository, &inflight.id).await.status,
        "sending"
    );
    repository
        .complete_delivery(&inflight.id, inflight.attempt, false)
        .await
        .unwrap();
    assert_eq!(
        delivery_record(&repository, &inflight.id).await.status,
        "cancelled"
    );
    assert_eq!(
        delivery_record(&repository, &delivered.id).await.status,
        "sent"
    );
    // 恢复通知仍能发送和重试，旧异常不能因投递队列继续工作而复活
    let recovery = repository.claim_delivery().await.unwrap().unwrap();
    assert_eq!(
        delivery_record(&repository, &recovery.id).await.kind,
        AlertNotificationKind::Recovered
    );
    repository
        .complete_delivery(&recovery.id, recovery.attempt, false)
        .await
        .unwrap();
    assert_eq!(
        delivery_record(&repository, &recovery.id).await.status,
        "pending"
    );
    db.close().await;
}

#[tokio::test]
async fn recovery_keeps_successful_inflight_delivery_and_cancels_unsent_recipients() {
    let Some(db) = TestDatabase::create("alerts_recovery_success").await else {
        return;
    };
    let repository = PgAlertRepository::new(db.pool.clone(), observability_query_budget());
    let settings = configure(&repository).await;
    let now = Utc::now();
    evaluate_at(&repository, &settings, now, 30).await;
    let inflight = repository.claim_delivery().await.unwrap().unwrap();
    recover(&repository, &settings, now).await;
    repository
        .complete_delivery(&inflight.id, inflight.attempt, true)
        .await
        .unwrap();
    let records = repository.status().await.unwrap().deliveries;
    let succeeded = records
        .iter()
        .find(|record| record.id == inflight.id)
        .unwrap();
    assert_eq!(succeeded.status, "sent");
    assert!(succeeded.sent_at.is_some());
    assert!(records.iter().any(|record| {
        record.kind == AlertNotificationKind::Triggered && record.status == "cancelled"
    }));
    assert!(
        records
            .iter()
            .filter(|record| record.kind == AlertNotificationKind::Recovered)
            .all(|record| record.status == "pending")
    );
    db.close().await;
}

#[tokio::test]
async fn recovery_cancels_alerts_that_failed_before_the_recovery_commit() {
    let Some(db) = TestDatabase::create("alerts_failure_before_recovery").await else {
        return;
    };
    let repository = PgAlertRepository::new(db.pool.clone(), observability_query_budget());
    let settings = configure(&repository).await;
    let now = Utc::now();
    evaluate_at(&repository, &settings, now, 30).await;
    let delivery = repository.claim_delivery().await.unwrap().unwrap();
    repository
        .complete_delivery(&delivery.id, delivery.attempt, false)
        .await
        .unwrap();
    assert_eq!(
        delivery_record(&repository, &delivery.id).await.status,
        "pending"
    );
    recover(&repository, &settings, now).await;
    assert_eq!(
        delivery_record(&repository, &delivery.id).await.status,
        "cancelled"
    );
    db.close().await;
}

#[tokio::test]
async fn recovered_events_never_reclaim_expired_trigger_or_reminder_deliveries() {
    let Some(db) = TestDatabase::create("alerts_recovered_claim").await else {
        return;
    };
    let repository = PgAlertRepository::new(db.pool.clone(), observability_query_budget());
    let settings = configure(&repository).await;
    let now = Utc::now();
    evaluate_at(&repository, &settings, now, 30).await;
    let triggered = repository.claim_delivery().await.unwrap().unwrap();
    // 第一条仍在发送，另一收件人的通知已确认成功
    let second = repository.claim_delivery().await.unwrap().unwrap();
    repository
        .complete_delivery(&second.id, second.attempt, true)
        .await
        .unwrap();
    let reminder_at = now + TimeDelta::minutes(30);
    evaluate_at(&repository, &settings, reminder_at, 30).await;
    let reminder = repository.claim_delivery().await.unwrap().unwrap();
    assert_eq!(
        delivery_record(&repository, &reminder.id).await.kind,
        AlertNotificationKind::Reminder
    );
    recover(&repository, &settings, reminder_at).await;
    sqlx::query("update alert_notifications set claimed_until=now()-interval '1 second' where id in ($1,$2)")
        .bind(&triggered.id).bind(&reminder.id).execute(&db.pool).await.unwrap();
    // 模拟重启，使用新的仓储实例沿持久化状态恢复
    let restarted = PgAlertRepository::new(db.pool.clone(), observability_query_budget());
    while let Some(delivery) = restarted.claim_delivery().await.unwrap() {
        assert_eq!(
            delivery_record(&restarted, &delivery.id).await.kind,
            AlertNotificationKind::Recovered
        );
        restarted
            .complete_delivery(&delivery.id, delivery.attempt, true)
            .await
            .unwrap();
    }
    for id in [&triggered.id, &reminder.id] {
        let record = delivery_record(&restarted, id).await;
        assert_eq!(record.status, "cancelled");
        assert_eq!(record.attempts, 1);
        // 过期投递的迟到结果不能覆盖取消事实
        restarted.complete_delivery(id, 1, true).await.unwrap();
        assert_eq!(delivery_record(&restarted, id).await.status, "cancelled");
    }
    db.close().await;
}

#[tokio::test]
async fn disabling_settings_cancels_inflight_and_ignores_late_results() {
    let Some(db) = TestDatabase::create("alerts_disabled_claim").await else {
        return;
    };
    let repository = PgAlertRepository::new(db.pool.clone(), observability_query_budget());
    let settings = configure(&repository).await;
    evaluate_at(&repository, &settings, Utc::now(), 30).await;
    let inflight = repository.claim_delivery().await.unwrap().unwrap();
    let mut values = settings.values;
    values.enabled = false;
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
        .unwrap();
    repository
        .complete_delivery(&inflight.id, inflight.attempt, true)
        .await
        .unwrap();
    assert_eq!(
        delivery_record(&repository, &inflight.id).await.status,
        "cancelled"
    );
    assert!(repository.claim_delivery().await.unwrap().is_none());
    db.close().await;
}
