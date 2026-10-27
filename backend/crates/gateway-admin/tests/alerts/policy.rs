//! 告警阈值、防抖、恢复、数据缺口与管理用例隔离测试

use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use gateway_admin::{
    alerts::policy::{evaluate, notification_text},
    model::{AdminError, MutationContext, alerts::*},
    ports::{
        alerts::{AlertRepository, EmailSender},
        store::{AdminStoreError, AdminStoreErrorKind, AdminStoreResult},
    },
};

pub(crate) struct TestAlertPorts;

#[async_trait]
impl AlertRepository for TestAlertPorts {
    async fn settings(&self) -> AdminStoreResult<AlertSettings> {
        Err(unused())
    }
    async fn update_settings(
        &self,
        _: UpdateAlertSettings,
        _: &MutationContext,
    ) -> AdminStoreResult<AlertSettings> {
        Err(unused())
    }
    async fn status(&self) -> AdminStoreResult<AlertStatus> {
        Err(unused())
    }
    async fn runtime(&self) -> AdminStoreResult<AlertRuntime> {
        Err(unused())
    }
    async fn sample(&self, _: DateTime<Utc>, _: DateTime<Utc>) -> AdminStoreResult<AlertSample> {
        Err(unused())
    }
    async fn commit_evaluation(
        &self,
        _: u64,
        _: u64,
        _: AlertEvaluation,
        _: String,
        _: String,
    ) -> AdminStoreResult<bool> {
        Err(unused())
    }
    async fn queue_test(&self, _: u64, _: &MutationContext) -> AdminStoreResult<()> {
        Err(unused())
    }
    async fn claim_delivery(&self) -> AdminStoreResult<Option<AlertDelivery>> {
        Err(unused())
    }
    async fn complete_delivery(&self, _: &str, _: u32, _: bool) -> AdminStoreResult<()> {
        Err(unused())
    }
}

#[async_trait]
impl EmailSender for TestAlertPorts {
    fn validate_addresses(&self, _: &AlertSettingsValues) -> Result<(), AdminError> {
        Ok(())
    }
    async fn send(&self, _: &AlertDelivery) -> Result<(), EmailSendError> {
        Err(EmailSendError)
    }
}

fn unused() -> AdminStoreError {
    AdminStoreError::new(AdminStoreErrorKind::Unavailable, "alerts test", "not used")
}

fn values() -> AlertSettingsValues {
    AlertSettingsValues {
        enabled: true,
        ..AlertSettingsValues::default()
    }
}

fn sample(now: DateTime<Utc>, failures: u64) -> AlertSample {
    AlertSample {
        window_start: now - TimeDelta::minutes(5),
        window_end: now,
        requests: 100,
        failures,
    }
}

fn tick(previous: &AlertRuntime, now: DateTime<Utc>, failures: u64) -> AlertEvaluation {
    evaluate(
        &values(),
        previous,
        Some(sample(now, failures)),
        now,
        "new-event".to_owned(),
    )
}

fn firing(now: DateTime<Utc>) -> AlertRuntime {
    let first = tick(&AlertRuntime::default(), now - TimeDelta::minutes(1), 20);
    tick(&first.runtime, now, 20).runtime
}

#[test]
fn defaults_are_disabled_and_require_a_delivery_configuration_before_enabling() {
    let settings = AlertSettingsValues::default();
    assert!(!settings.enabled);
    assert!(settings.validate().is_ok());
    assert!(settings.validate_delivery(false).is_err());
}

#[test]
fn fires_only_after_consecutive_evaluations_and_includes_the_exact_threshold() {
    let now = Utc::now();
    let first = tick(&AlertRuntime::default(), now, 20);
    assert_eq!(first.runtime.phase, AlertPhase::Pending);
    assert_eq!(first.notification, None);
    let second = tick(&first.runtime, now + TimeDelta::minutes(1), 20);
    assert_eq!(second.runtime.phase, AlertPhase::Firing);
    assert_eq!(second.notification, Some(AlertNotificationKind::Triggered));
    assert_eq!(second.runtime.active_event_id.as_deref(), Some("new-event"));
}

#[test]
fn a_successful_evaluation_resets_pending_abnormal_count() {
    let now = Utc::now();
    let first = tick(&AlertRuntime::default(), now, 30);
    let success = tick(&first.runtime, now + TimeDelta::minutes(1), 19);
    assert_eq!(success.runtime.phase, AlertPhase::Normal);
    let later = tick(&success.runtime, now + TimeDelta::minutes(2), 30);
    assert_eq!(later.runtime.phase, AlertPhase::Pending);
}

#[test]
fn unavailable_sample_does_not_recover_an_open_incident() {
    let now = Utc::now();
    let result = evaluate(
        &values(),
        &firing(now),
        None,
        now + TimeDelta::minutes(1),
        "unused".to_owned(),
    );
    assert_eq!(result.runtime.phase, AlertPhase::Firing);
    assert_eq!(
        result.runtime.evaluation_status,
        AlertEvaluationStatus::Unavailable
    );
    assert_eq!(result.notification, None);
}

#[test]
fn small_samples_and_zero_requests_never_trigger_or_recover() {
    let now = Utc::now();
    for requests in [0, 1, 19] {
        for previous in [AlertRuntime::default(), firing(now)] {
            let result = evaluate(
                &values(),
                &previous,
                Some(AlertSample {
                    requests,
                    failures: requests,
                    ..sample(now, 0)
                }),
                now + TimeDelta::minutes(1),
                "unused".to_owned(),
            );
            assert_eq!(
                result.runtime.evaluation_status,
                AlertEvaluationStatus::InsufficientSamples
            );
            assert_eq!(result.notification, None);
            if previous.phase == AlertPhase::Firing {
                assert_eq!(result.runtime.phase, AlertPhase::Firing);
            }
        }
    }
}

#[test]
fn low_sample_and_query_failure_break_consecutive_evaluations() {
    let now = Utc::now();
    let first = tick(&AlertRuntime::default(), now, 30);
    for sample in [
        None,
        Some(AlertSample {
            requests: 0,
            failures: 0,
            ..sample(now, 0)
        }),
    ] {
        let gap = evaluate(
            &values(),
            &first.runtime,
            sample,
            now + TimeDelta::minutes(1),
            "unused".to_owned(),
        );
        let next = tick(&gap.runtime, now + TimeDelta::minutes(2), 30);
        assert_eq!(next.runtime.phase, AlertPhase::Pending);
        assert_eq!(next.runtime.abnormal_count, 1);
    }
}

#[test]
fn long_check_gaps_and_clock_rollback_break_consecutive_evaluations() {
    let now = Utc::now();
    let first = tick(&AlertRuntime::default(), now, 30);
    for at in [now + TimeDelta::minutes(3), now - TimeDelta::minutes(1)] {
        let next = tick(&first.runtime, at, 30);
        assert_eq!(next.runtime.phase, AlertPhase::Pending);
        assert_eq!(next.runtime.abnormal_count, 1);
    }
}

#[test]
fn recovery_requires_two_values_strictly_below_the_recovery_threshold() {
    let now = Utc::now();
    let previous = firing(now);
    let equal = tick(&previous, now + TimeDelta::minutes(1), 10);
    assert_eq!(equal.runtime.recovery_count, 0);
    let low = tick(&equal.runtime, now + TimeDelta::minutes(2), 9);
    assert_eq!(low.runtime.phase, AlertPhase::Firing);
    let recovered = tick(&low.runtime, now + TimeDelta::minutes(3), 0);
    assert_eq!(recovered.runtime.phase, AlertPhase::Normal);
    assert_eq!(
        recovered.notification,
        Some(AlertNotificationKind::Recovered)
    );
    assert_eq!(recovered.event_id.as_deref(), Some("new-event"));
    assert!(recovered.runtime.active_event_id.is_none());
}

#[test]
fn failed_check_breaks_recovery_progress_without_clearing_the_incident() {
    let now = Utc::now();
    let first = tick(&firing(now), now + TimeDelta::minutes(1), 0);
    let gap = evaluate(
        &values(),
        &first.runtime,
        None,
        now + TimeDelta::minutes(2),
        "unused".to_owned(),
    );
    let next = tick(&gap.runtime, now + TimeDelta::minutes(3), 0);
    assert_eq!(next.runtime.phase, AlertPhase::Firing);
    assert_eq!(next.runtime.recovery_count, 1);
}

#[test]
fn reminders_keep_the_same_event_and_respect_the_persisted_interval() {
    let now = Utc::now();
    let mut previous = firing(now);
    for minute in 1..30 {
        let next = tick(&previous, now + TimeDelta::minutes(minute), 30);
        assert_eq!(next.notification, None);
        previous = next.runtime;
    }
    let reminder = tick(&previous, now + TimeDelta::minutes(30), 30);
    assert_eq!(reminder.notification, Some(AlertNotificationKind::Reminder));
    assert_eq!(reminder.event_id.as_deref(), Some("new-event"));
}

#[test]
fn reminders_can_be_disabled_without_disabling_recovery() {
    let now = Utc::now();
    let mut settings = values();
    settings.reminder_minutes = None;
    let previous = firing(now);
    let next = evaluate(
        &settings,
        &previous,
        Some(sample(now, 30)),
        now + TimeDelta::hours(1),
        "unused".to_owned(),
    );
    assert_eq!(next.notification, None);
    assert_eq!(next.runtime.phase, AlertPhase::Firing);
}

#[test]
fn disabling_does_not_emit_a_recovery_notification() {
    let now = Utc::now();
    let next = evaluate(
        &AlertSettingsValues::default(),
        &firing(now),
        None,
        now,
        "unused".to_owned(),
    );
    assert_eq!(next.notification, None);
    assert_eq!(
        next.runtime.evaluation_status,
        AlertEvaluationStatus::Disabled
    );
    assert_eq!(next.runtime.phase, AlertPhase::Normal);
}

#[test]
fn persisted_state_can_resume_without_repeating_the_first_notification() {
    let now = Utc::now();
    let persisted = serde_json::to_string(&firing(now)).unwrap();
    let restored = serde_json::from_str(&persisted).unwrap();
    let next = tick(&restored, now + TimeDelta::minutes(1), 40);
    assert_eq!(next.notification, None);
    assert_eq!(next.runtime.phase, AlertPhase::Firing);
}

#[test]
fn threshold_comparison_uses_integer_cross_multiplication() {
    let now = Utc::now();
    let record = AlertSample {
        requests: u64::MAX,
        failures: u64::MAX / 2,
        ..sample(now, 0)
    };
    assert!(!record.at_least(50));
    assert!(record.at_least(49));
    assert_eq!(
        AlertSample {
            requests: 0,
            failures: 0,
            ..sample(now, 0)
        }
        .failure_rate_percent(),
        None
    );
}

#[test]
fn configuration_rejects_unsafe_or_unrecoverable_values() {
    let mut config = values();
    config.recovery_threshold_percent = 0;
    assert!(config.validate().is_err());
    config = values();
    config.recovery_threshold_percent = 20;
    assert!(config.validate().is_err());
    config = values();
    config.smtp_host = "smtp.example.com\r\nsecret".to_owned();
    assert!(config.validate().is_err());
    config = values();
    config.recipients = vec!["person@example.com".to_owned(); 11];
    assert!(config.validate().is_err());
}

#[test]
fn configuration_normalizes_and_deduplicates_recipients() {
    let mut config = values();
    config.recipients = vec![" a@example.com ".to_owned(), "a@example.com".to_owned()];
    config.normalize();
    assert_eq!(config.recipients, vec!["a@example.com"]);
}

#[test]
fn smtp_password_never_appears_in_debug_output_or_notification_text() {
    let now = Utc::now();
    let settings = AlertSettings {
        revision: 1,
        values: values(),
        password: Some(secrecy::SecretString::from("PRIVATE_SMTP_PASSWORD")),
        updated_at: now,
    };
    assert!(!format!("{settings:?}").contains("PRIVATE_SMTP_PASSWORD"));
    let result = tick(&firing(now), now + TimeDelta::minutes(30), 40);
    let (subject, body) = notification_text(&settings.values, &result);
    assert!(subject.contains("持续告警"));
    assert!(!body.contains("PRIVATE_SMTP_PASSWORD"));
}
