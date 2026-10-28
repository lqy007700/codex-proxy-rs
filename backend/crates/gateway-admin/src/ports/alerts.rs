//! 邮件告警的事务持久化、只读采样与外部邮件能力端口

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use super::store::AdminStoreResult;
use crate::model::{AdminError, MutationContext, alerts::*};

#[async_trait]
pub trait AlertRepository: Send + Sync {
    async fn quota_accounts(
        &self,
        after: Option<String>,
    ) -> AdminStoreResult<Vec<QuotaWarningAccount>> {
        let _ = after;
        Err(super::store::AdminStoreError::new(
            super::store::AdminStoreErrorKind::Unavailable,
            "quota alerts",
            "quota observations unavailable",
        ))
    }
    async fn quota_states(&self) -> AdminStoreResult<Vec<QuotaWarningState>> {
        Ok(Vec::new())
    }
    async fn quota_failed(
        &self,
        _revision: u64,
        _previous_check: Option<DateTime<Utc>>,
    ) -> AdminStoreResult<()> {
        Ok(())
    }
    async fn quota_status(&self) -> AdminStoreResult<QuotaWarningStatus> {
        Ok(QuotaWarningStatus::default())
    }
    async fn commit_quota_evaluation(
        &self,
        revision: u64,
        previous_check: Option<DateTime<Utc>>,
        evaluation: QuotaWarningEvaluation,
    ) -> AdminStoreResult<bool> {
        let _ = (revision, previous_check, evaluation);
        Err(super::store::AdminStoreError::new(
            super::store::AdminStoreErrorKind::Unavailable,
            "quota alerts",
            "quota persistence unavailable",
        ))
    }
    async fn settings(&self) -> AdminStoreResult<AlertSettings>;
    async fn update_settings(
        &self,
        command: UpdateAlertSettings,
        context: &MutationContext,
    ) -> AdminStoreResult<AlertSettings>;
    async fn status(&self) -> AdminStoreResult<AlertStatus>;
    async fn runtime(&self) -> AdminStoreResult<AlertRuntime>;
    async fn sample(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> AdminStoreResult<AlertSample>;
    async fn commit_evaluation(
        &self,
        expected_revision: u64,
        expected_generation: u64,
        evaluation: AlertEvaluation,
        subject: String,
        body: String,
    ) -> AdminStoreResult<bool>;
    async fn queue_test(
        &self,
        expected_revision: u64,
        context: &MutationContext,
    ) -> AdminStoreResult<()>;
    async fn claim_delivery(&self) -> AdminStoreResult<Option<AlertDelivery>>;
    async fn complete_delivery(
        &self,
        id: &str,
        attempt: u32,
        succeeded: bool,
    ) -> AdminStoreResult<()>;
}

#[async_trait]
pub trait EmailSender: Send + Sync {
    /// 仅检查邮件地址，不连接 SMTP 或访问网络
    fn validate_addresses(&self, values: &AlertSettingsValues) -> Result<(), AdminError>;
    async fn send(&self, delivery: &AlertDelivery) -> Result<(), EmailSendError>;
}
