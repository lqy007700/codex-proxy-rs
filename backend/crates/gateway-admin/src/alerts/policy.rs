//! 模型请求失败率告警的纯状态转换，不执行存储与邮件副作用

use chrono::{DateTime, TimeDelta, Utc};

use crate::model::alerts::*;

/// 失败采样和低样本中断连续性，但不能把未确认的故障当作恢复
pub fn evaluate(
    values: &AlertSettingsValues,
    previous: &AlertRuntime,
    sample: Option<AlertSample>,
    now: DateTime<Utc>,
    new_event_id: String,
) -> AlertEvaluation {
    let mut runtime = previous.clone();
    runtime.generation = runtime.generation.saturating_add(1);
    runtime.checked_at = Some(now);
    runtime.sample = sample;
    let mut result = AlertEvaluation {
        event_id: previous.active_event_id.clone(),
        runtime,
        notification: None,
    };
    let runtime = &mut result.runtime;
    if !values.enabled {
        runtime.phase = AlertPhase::Normal;
        runtime.evaluation_status = AlertEvaluationStatus::Disabled;
        runtime.abnormal_count = 0;
        runtime.recovery_count = 0;
        runtime.active_event_id = None;
        runtime.opened_at = None;
        return result;
    }
    if previous.checked_at.is_none_or(|last| {
        now.signed_duration_since(last) > TimeDelta::seconds(2 * EVALUATION_INTERVAL_SECONDS + 10)
            || now <= last
    }) {
        runtime.abnormal_count = 0;
        runtime.recovery_count = 0;
    }
    let Some(sample) = &runtime.sample else {
        runtime.evaluation_status = AlertEvaluationStatus::Unavailable;
        reset_pending(runtime);
        return result;
    };
    if sample.requests < u64::from(values.minimum_requests) {
        runtime.evaluation_status = AlertEvaluationStatus::InsufficientSamples;
        reset_pending(runtime);
        return result;
    }
    runtime.evaluation_status = AlertEvaluationStatus::Ready;
    if runtime.phase == AlertPhase::Firing {
        runtime.recovery_count = if !sample.at_least(values.recovery_threshold_percent) {
            runtime.recovery_count.saturating_add(1)
        } else {
            0
        };
        if runtime.recovery_count >= 2 {
            runtime.phase = AlertPhase::Normal;
            runtime.recovery_count = 0;
            runtime.abnormal_count = 0;
            runtime.active_event_id = None;
            result.notification = Some(AlertNotificationKind::Recovered);
        } else if values.reminder_minutes.is_some_and(|minutes| {
            runtime.last_notification_at.is_some_and(|last| {
                now.signed_duration_since(last) >= TimeDelta::minutes(i64::from(minutes))
            })
        }) {
            result.notification = Some(AlertNotificationKind::Reminder);
        }
    } else if sample.at_least(values.failure_threshold_percent) {
        runtime.abnormal_count = runtime.abnormal_count.saturating_add(1);
        runtime.phase = AlertPhase::Pending;
        if runtime.abnormal_count >= values.consecutive_evaluations {
            runtime.phase = AlertPhase::Firing;
            runtime.active_event_id = Some(new_event_id.clone());
            runtime.opened_at = Some(now);
            result.event_id = Some(new_event_id);
            result.notification = Some(AlertNotificationKind::Triggered);
        }
    } else {
        runtime.phase = AlertPhase::Normal;
        runtime.abnormal_count = 0;
    }
    if result.notification.is_some() {
        runtime.last_notification_at = Some(now);
    }
    result
}

fn reset_pending(runtime: &mut AlertRuntime) {
    runtime.abnormal_count = 0;
    runtime.recovery_count = 0;
    if runtime.phase == AlertPhase::Pending {
        runtime.phase = AlertPhase::Normal;
    }
}

/// 邮件仅包含受控统计摘要，避免上游正文或请求内容进入通知
pub fn notification_text(
    values: &AlertSettingsValues,
    evaluation: &AlertEvaluation,
) -> (String, String) {
    let label = match evaluation.notification {
        Some(AlertNotificationKind::Recovered) => "恢复",
        Some(AlertNotificationKind::Reminder) => "持续告警",
        _ => "告警",
    };
    let Some(sample) = &evaluation.runtime.sample else {
        return (String::new(), String::new());
    };
    let rate = sample.failure_rate_percent().unwrap_or_default();
    let subject = format!("[Codex Proxy][{label}] 全站模型请求失败率 {rate:.1}%");
    let duration = evaluation.runtime.opened_at.map_or(0, |opened| {
        sample
            .window_end
            .signed_duration_since(opened)
            .num_seconds()
            .max(0)
    });
    let body = format!(
        "全站模型请求失败率{label}\n\n统计窗口（UTC）: {} 至 {}\n有效请求: {}\n失败请求: {}\n失败率: {:.1}%\n触发阈值: {}%\n恢复阈值: {}%\n异常持续: {} 秒\n\n请登录管理端，在系统设置 → 告警通知中查看状态\n本指标基于异步记录的模型请求终态，不覆盖所有入口拒绝\n",
        sample.window_start.to_rfc3339(),
        sample.window_end.to_rfc3339(),
        sample.requests,
        sample.failures,
        rate,
        values.failure_threshold_percent,
        values.recovery_threshold_percent,
        duration,
    );
    (subject, body)
}
