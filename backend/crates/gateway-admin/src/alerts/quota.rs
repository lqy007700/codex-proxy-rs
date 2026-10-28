//! 已同步额度的周期判定和合并邮件，不查询上游或改变账号调度

use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, BTreeSet};

use crate::model::alerts::*;

#[must_use]
pub fn evaluate(
    settings: &QuotaWarningSettings,
    accounts: Vec<(QuotaWarningAccount, Option<Vec<QuotaWarningWindow>>)>,
    previous: &[QuotaWarningState],
    now: DateTime<Utc>,
) -> QuotaWarningEvaluation {
    let previous: BTreeMap<_, _> = previous
        .iter()
        .map(|state| {
            (
                (state.account_id.as_str(), state.window_key.as_str()),
                state,
            )
        })
        .collect();
    let mut result = QuotaWarningEvaluation {
        status: QuotaWarningStatus {
            checked_at: Some(now),
            ..QuotaWarningStatus::default()
        },
        ..QuotaWarningEvaluation::default()
    };
    for (account, windows) in accounts {
        result.status.accounts += 1;
        let Some(windows) = windows.filter(|windows| !windows.is_empty()) else {
            result.status.unavailable_accounts += 1;
            continue;
        };
        let mut keys = BTreeSet::new();
        for window in windows {
            if !keys.insert(window.key.clone()) {
                continue;
            }
            let valid = account
                .observed_at
                .zip(window.reset_at)
                .zip(window.used_percent)
                .filter(|((observed, reset), used)| {
                    *observed <= now
                        && *observed < *reset
                        && *reset > now
                        && used.is_finite()
                        && (0.0..=100.0).contains(used)
                });
            let Some(((observed, reset), used)) = valid else {
                result.status.unavailable_windows += 1;
                continue;
            };
            let old = previous.get(&(account.id.as_str(), window.key.as_str()));
            // 只有跨过旧周期边界的新观测才能重新计数，reset 修正和使用比例抖动不重发
            let new_cycle =
                old.is_none_or(|old| observed >= old.cycle_reset_at && reset > old.cycle_reset_at);
            let mut state = match old.filter(|_| !new_cycle) {
                Some(old) => (*old).clone(),
                None => QuotaWarningState {
                    account_id: account.id.clone(),
                    window_key: window.key.clone(),
                    cycle_reset_at: reset,
                    notified: false,
                },
            };
            let threshold = match window.period {
                QuotaWarningPeriod::ShortTerm => settings.short_term_threshold_percent,
                QuotaWarningPeriod::Weekly => settings.weekly_threshold_percent,
            };
            if state.notified {
                result.status.notified_windows += 1;
            } else if used >= f64::from(threshold) {
                state.notified = true;
                result.status.pending_windows += 1;
                result.items.push(QuotaWarningItem {
                    account: account.clone(),
                    window,
                    threshold,
                });
            }
            result.states.push(state);
        }
    }
    result
}

#[must_use]
pub fn notification_text(evaluation: &QuotaWarningEvaluation) -> (String, String) {
    use std::fmt::Write as _;
    let accounts: BTreeSet<_> = evaluation
        .items
        .iter()
        .map(|item| &item.account.id)
        .collect();
    let subject = format!(
        "[Codex Proxy][额度预警] {} 个账号、{} 个额度窗口达到阈值",
        accounts.len(),
        evaluation.items.len()
    );
    let mut body = format!(
        "检查时间（UTC）: {}\n",
        evaluation
            .status
            .checked_at
            .map(|at| at.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_default()
    );
    let mut last_account = None;
    for item in &evaluation.items {
        if last_account != Some(item.account.id.as_str()) {
            let label = item
                .account
                .email
                .as_deref()
                .filter(|email| !email.is_empty())
                .map_or_else(
                    || format!("{} ({})", item.account.name, item.account.id),
                    str::to_owned,
                );
            let _ = write!(
                body,
                "\n账号: {label}\n套餐: {}\n",
                item.account.plan_type.as_deref().unwrap_or("未知")
            );
            last_account = Some(item.account.id.as_str());
        }
        let label = match item.window.period {
            QuotaWarningPeriod::ShortTerm => "5 小时额度",
            QuotaWarningPeriod::Weekly => "周额度",
        };
        let _ = writeln!(
            body,
            "{label}: 已使用 {:.1}%，预警阈值 {}%",
            item.window.used_percent.unwrap_or_default(),
            item.threshold
        );
        if let Some(reset) = item.window.reset_at {
            let _ = writeln!(
                body,
                "预计重置（UTC）: {}",
                reset.format("%Y-%m-%d %H:%M:%S")
            );
        }
        if let Some(observed) = item.account.observed_at {
            let _ = writeln!(
                body,
                "额度更新时间（UTC）: {}",
                observed.format("%Y-%m-%d %H:%M:%S")
            );
        }
    }
    body.push_str("\n每个额度窗口每周期仅提醒一次\n本通知基于网关最近同步的上游额度\n");
    (subject, body)
}
