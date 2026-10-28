//! 额度窗口独立阈值、自然周期去重、旧快照和合并通知测试

use chrono::{DateTime, TimeDelta, Utc};
use gateway_admin::{
    alerts::quota::{evaluate, notification_text},
    model::alerts::*,
};

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-28T08:00:00Z")
        .unwrap()
        .to_utc()
}
fn account(id: &str) -> QuotaWarningAccount {
    QuotaWarningAccount {
        id: id.to_owned(),
        name: "示例账号".to_owned(),
        email: Some(format!("{id}@example.test")),
        plan_type: Some("plus".to_owned()),
        observed_at: Some(now()),
        credential_revision: 1,
        document: None,
    }
}
fn window(period: QuotaWarningPeriod, used: f64) -> QuotaWarningWindow {
    let (key, hours) = match period {
        QuotaWarningPeriod::ShortTerm => ("codex:18000s", 5),
        QuotaWarningPeriod::Weekly => ("codex:604800s", 168),
    };
    QuotaWarningWindow {
        key: key.to_owned(),
        period,
        used_percent: Some(used),
        reset_at: Some(now() + TimeDelta::hours(hours)),
    }
}

#[test]
fn threshold_is_inclusive_and_two_windows_have_independent_thresholds() {
    let settings = QuotaWarningSettings {
        enabled: true,
        short_term_threshold_percent: 80,
        weekly_threshold_percent: 90,
    };
    let result = evaluate(
        &settings,
        vec![(
            account("a"),
            Some(vec![
                window(QuotaWarningPeriod::ShortTerm, 79.99),
                window(QuotaWarningPeriod::Weekly, 90.0),
            ]),
        )],
        &[],
        now(),
    );
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].window.period, QuotaWarningPeriod::Weekly);
}

#[test]
fn same_cycle_never_repeats_after_restart_threshold_changes_or_reset_corrections() {
    let settings = QuotaWarningSettings::default();
    let first = evaluate(
        &settings,
        vec![(
            account("a"),
            Some(vec![window(QuotaWarningPeriod::ShortTerm, 80.0)]),
        )],
        &[],
        now(),
    );
    for used in [75.0, 80.0, 95.0, 100.0] {
        let mut changed = window(QuotaWarningPeriod::ShortTerm, used);
        changed.reset_at = changed.reset_at.map(|at| at + TimeDelta::minutes(2));
        let result = evaluate(
            &settings,
            vec![(account("a"), Some(vec![changed]))],
            &first.states,
            now() + TimeDelta::minutes(10),
        );
        assert!(result.items.is_empty());
        assert_eq!(
            result.states[0].cycle_reset_at,
            first.states[0].cycle_reset_at
        );
    }
}

#[test]
fn fresh_observation_after_short_cycle_rearms_short_window_without_rearming_weekly() {
    let settings = QuotaWarningSettings::default();
    let first = evaluate(
        &settings,
        vec![(
            account("a"),
            Some(vec![
                window(QuotaWarningPeriod::ShortTerm, 85.0),
                window(QuotaWarningPeriod::Weekly, 85.0),
            ]),
        )],
        &[],
        now(),
    );
    let later = now() + TimeDelta::hours(6);
    let mut a = account("a");
    a.observed_at = Some(later);
    let mut short = window(QuotaWarningPeriod::ShortTerm, 85.0);
    short.reset_at = Some(later + TimeDelta::hours(4));
    let result = evaluate(
        &settings,
        vec![(
            a,
            Some(vec![short, window(QuotaWarningPeriod::Weekly, 95.0)]),
        )],
        &first.states,
        later,
    );
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].window.period, QuotaWarningPeriod::ShortTerm);
}

#[test]
fn missing_expired_future_and_invalid_observations_do_not_consume_notification_eligibility() {
    let settings = QuotaWarningSettings::default();
    let mut invalid = Vec::new();
    for used in [f64::NAN, -1.0, f64::INFINITY] {
        invalid.push(window(QuotaWarningPeriod::ShortTerm, used));
    }
    let mut expired = window(QuotaWarningPeriod::Weekly, 90.0);
    expired.reset_at = Some(now());
    invalid.push(expired);
    let mut missing = window(QuotaWarningPeriod::Weekly, 90.0);
    missing.reset_at = None;
    invalid.push(missing);
    for w in invalid {
        let result = evaluate(&settings, vec![(account("a"), Some(vec![w]))], &[], now());
        assert!(result.items.is_empty());
        assert!(result.states.is_empty());
    }
    let mut future = account("a");
    future.observed_at = Some(now() + TimeDelta::seconds(1));
    let result = evaluate(
        &settings,
        vec![
            (
                future,
                Some(vec![window(QuotaWarningPeriod::ShortTerm, 90.0)]),
            ),
            (account("b"), None),
        ],
        &[],
        now(),
    );
    assert!(result.items.is_empty());
    assert_eq!(result.status.unavailable_accounts, 1);
}

#[test]
fn one_digest_contains_all_accounts_both_periods_and_readable_utc_times() {
    let result = evaluate(
        &QuotaWarningSettings::default(),
        vec![
            (
                account("a"),
                Some(vec![
                    window(QuotaWarningPeriod::ShortTerm, 80.0),
                    window(QuotaWarningPeriod::Weekly, 95.0),
                ]),
            ),
            (
                account("b"),
                Some(vec![window(QuotaWarningPeriod::Weekly, 90.0)]),
            ),
        ],
        &[],
        now(),
    );
    let (subject, body) = notification_text(&result);
    assert_eq!(
        subject,
        "[Codex Proxy][额度预警] 2 个账号、3 个额度窗口达到阈值"
    );
    assert_eq!(body.matches("账号: a@example.test").count(), 1);
    assert!(body.contains("5 小时额度: 已使用 80.0%"));
    assert!(body.contains("周额度: 已使用 95.0%"));
    assert!(body.contains("2026-10-28 08:00:00"));
    assert!(!body.contains("T08:"));
}
