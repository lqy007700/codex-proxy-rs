//! 单例邮件告警配置、请求终态采样与持久化评估状态

use chrono::{DateTime, Utc};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};

use super::AdminError;

pub const MAX_RECIPIENTS: usize = 10;
pub const MAX_PENDING_DELIVERIES: i64 = 100;
pub const MAX_DELIVERY_ATTEMPTS: u32 = 3;
pub const EVALUATION_INTERVAL_SECONDS: i64 = 60;
pub const QUOTA_EVALUATION_INTERVAL_SECONDS: i64 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmtpSecurity {
    Tls,
    StartTls,
}

/// 非秘密配置可以安全投影，密码由独立更新命令与存储字段持有
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertSettingsValues {
    pub enabled: bool,
    #[serde(default)]
    pub quota_warning: Option<QuotaWarningSettings>,
    pub window_minutes: u32,
    pub minimum_requests: u32,
    pub failure_threshold_percent: u32,
    pub consecutive_evaluations: u32,
    pub recovery_threshold_percent: u32,
    pub reminder_minutes: Option<u32>,
    pub recovery_notification: bool,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_security: SmtpSecurity,
    pub smtp_username: String,
    pub sender_address: String,
    pub sender_name: String,
    pub recipients: Vec<String>,
}

impl Default for AlertSettingsValues {
    fn default() -> Self {
        Self {
            enabled: false,
            quota_warning: Some(QuotaWarningSettings::default()),
            window_minutes: 5,
            minimum_requests: 20,
            failure_threshold_percent: 20,
            consecutive_evaluations: 2,
            recovery_threshold_percent: 10,
            reminder_minutes: Some(30),
            recovery_notification: true,
            smtp_host: String::new(),
            smtp_port: 465,
            smtp_security: SmtpSecurity::Tls,
            smtp_username: String::new(),
            sender_address: String::new(),
            sender_name: "Codex Proxy".to_owned(),
            recipients: Vec::new(),
        }
    }
}

impl AlertSettingsValues {
    #[must_use]
    pub fn quota_warning(&self) -> QuotaWarningSettings {
        self.quota_warning.clone().unwrap_or_default()
    }

    pub fn validate(&self) -> Result<(), AdminError> {
        let quota = self.quota_warning();
        if !(1..=100).contains(&quota.short_term_threshold_percent)
            || !(1..=100).contains(&quota.weekly_threshold_percent)
        {
            return Err(AdminError::invalid("额度预警阈值应为 1 至 100 的整数"));
        }
        if !(1..=60).contains(&self.window_minutes)
            || !(1..=1_000_000).contains(&self.minimum_requests)
            || !(1..=100).contains(&self.failure_threshold_percent)
            || self.recovery_threshold_percent == 0
            || self.recovery_threshold_percent >= self.failure_threshold_percent
            || !(1..=10).contains(&self.consecutive_evaluations)
            || self
                .reminder_minutes
                .is_some_and(|value| !(5..=1440).contains(&value))
            || self.smtp_port == 0
        {
            return Err(AdminError::invalid("告警周期、阈值或提醒间隔不符合要求"));
        }
        if self.recipients.len() > MAX_RECIPIENTS {
            return Err(AdminError::invalid("接收邮箱最多配置 10 个"));
        }
        for (text, maximum) in [
            (self.smtp_host.as_str(), 253),
            (self.smtp_username.as_str(), 320),
            (self.sender_address.as_str(), 320),
            (self.sender_name.as_str(), 128),
        ] {
            if text.len() > maximum || text.chars().any(char::is_control) {
                return Err(AdminError::invalid("邮件配置包含过长字段或控制字符"));
            }
        }
        if self
            .recipients
            .iter()
            .any(|value| value.len() > 320 || value.chars().any(char::is_control))
        {
            return Err(AdminError::invalid("接收邮箱格式不正确"));
        }
        if !self.smtp_host.is_empty()
            && (self.smtp_host.contains(['/', ':', '@', ' ', '\\'])
                || self.smtp_host.starts_with('.'))
        {
            return Err(AdminError::invalid("SMTP 服务器只填写主机名或 IPv4 地址"));
        }
        Ok(())
    }

    pub fn normalize(&mut self) {
        self.smtp_host = self.smtp_host.trim().to_owned();
        self.smtp_username = self.smtp_username.trim().to_owned();
        self.sender_address = self.sender_address.trim().to_owned();
        self.sender_name = self.sender_name.trim().to_owned();
        self.recipients = self
            .recipients
            .iter()
            .map(|value| value.trim().to_owned())
            .collect();
        self.recipients.sort();
        self.recipients.dedup();
    }

    pub fn validate_delivery(&self, password_configured: bool) -> Result<(), AdminError> {
        self.validate()?;
        if self.smtp_host.is_empty() || self.sender_address.is_empty() || self.recipients.is_empty()
        {
            return Err(AdminError::invalid(
                "请先配置 SMTP 服务器、发件人和接收邮箱",
            ));
        }
        if !self.smtp_username.is_empty() && !password_configured {
            return Err(AdminError::invalid("请先配置 SMTP 密码或授权码"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct AlertSettings {
    pub revision: u64,
    pub values: AlertSettingsValues,
    pub password: Option<SecretString>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct UpdateAlertSettings {
    pub expected_revision: u64,
    pub values: AlertSettingsValues,
    pub password: Option<SecretString>,
    pub clear_password: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertSample {
    pub window_start: DateTime<Utc>,
    pub window_end: DateTime<Utc>,
    pub requests: u64,
    pub failures: u64,
}

impl AlertSample {
    #[must_use]
    pub fn failure_rate_percent(&self) -> Option<f64> {
        (self.requests > 0).then(|| self.failures as f64 / self.requests as f64 * 100.0)
    }

    #[must_use]
    pub fn at_least(&self, percent: u32) -> bool {
        u128::from(self.failures) * 100 >= u128::from(self.requests) * u128::from(percent)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertPhase {
    #[default]
    Normal,
    Pending,
    Firing,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertEvaluationStatus {
    #[default]
    Disabled,
    Waiting,
    Ready,
    InsufficientSamples,
    Unavailable,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertRuntime {
    pub generation: u64,
    pub phase: AlertPhase,
    pub evaluation_status: AlertEvaluationStatus,
    pub abnormal_count: u32,
    pub recovery_count: u32,
    pub checked_at: Option<DateTime<Utc>>,
    pub sample: Option<AlertSample>,
    pub active_event_id: Option<String>,
    pub opened_at: Option<DateTime<Utc>>,
    pub last_notification_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertNotificationKind {
    Triggered,
    Reminder,
    Recovered,
    Test,
    QuotaWarning,
}

impl AlertNotificationKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Triggered => "triggered",
            Self::Reminder => "reminder",
            Self::Recovered => "recovered",
            Self::Test => "test",
            Self::QuotaWarning => "quota_warning",
        }
    }
}

/// 纯评估输出，由存储端在版本匹配的同一事务内提交事件与通知
#[derive(Debug, Clone)]
pub struct AlertEvaluation {
    pub runtime: AlertRuntime,
    pub notification: Option<AlertNotificationKind>,
    pub event_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertEvent {
    pub id: String,
    pub status: String,
    pub opened_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    pub sample: AlertSample,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertDeliveryRecord {
    pub id: String,
    pub kind: AlertNotificationKind,
    pub recipient: String,
    pub status: String,
    pub attempts: u32,
    pub created_at: DateTime<Utc>,
    pub sent_at: Option<DateTime<Utc>>,
    pub next_attempt_at: DateTime<Utc>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AlertStatus {
    pub quota_warning: QuotaWarningStatus,
    pub runtime: AlertRuntime,
    pub events: Vec<AlertEvent>,
    pub deliveries: Vec<AlertDeliveryRecord>,
}

/// 领取后即释放数据库资源，SMTP 网络调用只持有不可变配置与消息
#[derive(Debug)]
pub struct AlertDelivery {
    pub id: String,
    pub attempt: u32,
    pub settings: AlertSettings,
    pub recipient: String,
    pub subject: String,
    pub body: String,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("邮件投递失败")]
pub struct EmailSendError;

/// 两个窗口独立设置阈值，SMTP 与失败率规则共用
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct QuotaWarningSettings {
    pub enabled: bool,
    pub short_term_threshold_percent: u32,
    pub weekly_threshold_percent: u32,
}

impl Default for QuotaWarningSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            short_term_threshold_percent: 80,
            weekly_threshold_percent: 80,
        }
    }
}

/// 只搬运额度文档，Provider 独占私有字段的解释
#[derive(Debug, Clone)]
pub struct QuotaWarningAccount {
    pub id: String,
    pub name: String,
    pub email: Option<String>,
    pub plan_type: Option<String>,
    pub observed_at: Option<DateTime<Utc>>,
    pub credential_revision: u64,
    pub document: Option<crate::model::provider_credentials::ProviderDocument>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaWarningPeriod {
    ShortTerm,
    Weekly,
}

/// Provider 仅投影账号通用的 5 小时和周额度，不合并独立窗口
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWarningWindow {
    pub key: String,
    pub period: QuotaWarningPeriod,
    pub used_percent: Option<f64>,
    pub reset_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWarningState {
    pub account_id: String,
    pub window_key: String,
    pub cycle_reset_at: DateTime<Utc>,
    pub notified: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct QuotaWarningStatus {
    pub checked_at: Option<DateTime<Utc>>,
    pub accounts: u64,
    pub notified_windows: u64,
    pub pending_windows: u64,
    pub unavailable_accounts: u64,
    pub unavailable_windows: u64,
    pub error: bool,
}

#[derive(Debug, Clone)]
pub struct QuotaWarningItem {
    pub account: QuotaWarningAccount,
    pub window: QuotaWarningWindow,
    pub threshold: u32,
}

#[derive(Debug, Clone, Default)]
pub struct QuotaWarningEvaluation {
    pub status: QuotaWarningStatus,
    pub states: Vec<QuotaWarningState>,
    pub items: Vec<QuotaWarningItem>,
}
