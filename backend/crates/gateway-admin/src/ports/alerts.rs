//! 邮件告警的事务持久化、只读采样与外部邮件能力端口

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use super::store::AdminStoreResult;
use crate::model::{AdminError, MutationContext, alerts::*};

#[async_trait]
pub trait AlertRepository: Send + Sync {
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
