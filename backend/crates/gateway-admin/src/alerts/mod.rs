//! 邮件告警管理与后台任务组装，独立于请求配置和执行生命周期

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use chrono::{TimeDelta, Utc};
use gateway_core::task::{
    WorkerContribution, WorkerDefinitionError, WorkerId, WorkerKind, WorkerRegistration,
    WorkerRunnable, WorkerSchedule,
};
use secrecy::ExposeSecret as _;

use crate::{
    model::{AdminError, MutationContext, alerts::*},
    ports::alerts::{AlertRepository, EmailSender},
    use_case::map_store_error,
};

pub mod policy;
mod task;

#[async_trait]
pub trait AlertService: Send + Sync {
    async fn settings(&self) -> Result<AlertSettings, AdminError>;
    async fn update(
        &self,
        command: UpdateAlertSettings,
        context: &MutationContext,
    ) -> Result<AlertSettings, AdminError>;
    async fn status(&self) -> Result<AlertStatus, AdminError>;
    async fn preview(&self) -> Result<AlertSample, AdminError>;
    async fn test_email(
        &self,
        expected_revision: u64,
        context: &MutationContext,
    ) -> Result<(), AdminError>;
}

pub struct DefaultAlertService {
    repository: Arc<dyn AlertRepository>,
    sender: Arc<dyn EmailSender>,
}

impl DefaultAlertService {
    #[must_use]
    pub fn new(repository: Arc<dyn AlertRepository>, sender: Arc<dyn EmailSender>) -> Self {
        Self { repository, sender }
    }

    pub fn workers(self: &Arc<Self>) -> Result<Vec<WorkerContribution>, WorkerDefinitionError> {
        [
            (
                WorkerKind::AlertEvaluation,
                60,
                task::AlertTaskKind::Evaluate,
            ),
            (WorkerKind::AlertDelivery, 5, task::AlertTaskKind::Deliver),
        ]
        .into_iter()
        .map(|(kind, interval, task_kind)| {
            let id = WorkerId::try_new(kind, "email-alerts")?;
            let schedule = WorkerSchedule::try_new(
                Duration::from_secs(interval),
                Duration::from_secs(5),
                Duration::from_secs(60),
                Duration::from_secs(90),
                Duration::from_secs(30),
            )?;
            Ok(WorkerContribution::Registration(
                WorkerRegistration::try_new(
                    id,
                    WorkerRunnable::Scheduled {
                        schedule,
                        // 单副本任务由 Host 串行驱动，数据库版本与 outbox claim 保证恢复安全
                        // 不增加 Redis 故障对告警的依赖，也不持有业务请求使用的租约
                        lease: None,
                        task: Box::new(task::AlertTask {
                            service: self.clone(),
                            kind: task_kind,
                        }),
                    },
                )?,
            ))
        })
        .collect()
    }

    async fn evaluate_cycle(&self) -> Result<(), AdminError> {
        let settings = self.settings().await?;
        if !settings.values.enabled {
            return Ok(());
        }
        let previous = self.repository.runtime().await.map_err(store_error)?;
        let now = Utc::now();
        if previous.checked_at.is_some_and(|last| {
            let age = now.signed_duration_since(last);
            age >= TimeDelta::zero() && age < TimeDelta::seconds(EVALUATION_INTERVAL_SECONDS)
        }) {
            return Ok(());
        }
        let sample = self
            .repository
            .sample(
                now - TimeDelta::minutes(i64::from(settings.values.window_minutes)),
                now,
            )
            .await;
        let evaluation = policy::evaluate(
            &settings.values,
            &previous,
            sample.as_ref().ok().cloned(),
            now,
            format!("alert_{}", uuid::Uuid::now_v7().simple()),
        );
        let (subject, body) = policy::notification_text(&settings.values, &evaluation);
        self.repository
            .commit_evaluation(
                settings.revision,
                previous.generation,
                evaluation,
                subject,
                body,
            )
            .await
            .map_err(store_error)?;
        // 先持久化不可用状态，再由监督器保留安全故障信息，不能伪造成功评估
        sample.map(|_| ()).map_err(store_error)
    }

    async fn deliver_cycle(&self) -> Result<(), AdminError> {
        let Some(delivery) = self
            .repository
            .claim_delivery()
            .await
            .map_err(store_error)?
        else {
            return Ok(());
        };
        let sent = tokio::time::timeout(Duration::from_secs(20), self.sender.send(&delivery)).await;
        let succeeded = matches!(sent, Ok(Ok(())));
        self.repository
            .complete_delivery(&delivery.id, delivery.attempt, succeeded)
            .await
            .map_err(store_error)?;
        if !succeeded {
            return Err(AdminError::unavailable("邮件发送失败，将按重试策略处理"));
        }
        Ok(())
    }
}

#[async_trait]
impl AlertService for DefaultAlertService {
    async fn settings(&self) -> Result<AlertSettings, AdminError> {
        self.repository.settings().await.map_err(store_error)
    }

    async fn update(
        &self,
        mut command: UpdateAlertSettings,
        context: &MutationContext,
    ) -> Result<AlertSettings, AdminError> {
        command.values.normalize();
        command.values.validate()?;
        self.sender.validate_addresses(&command.values)?;
        if command.expected_revision == 0 || (command.clear_password && command.password.is_some())
        {
            return Err(AdminError::invalid("配置版本或密码更新方式不正确"));
        }
        if command.password.as_ref().is_some_and(|value| {
            value.expose_secret().is_empty() || value.expose_secret().len() > 4096
        }) {
            return Err(AdminError::invalid("SMTP 密码长度不符合要求"));
        }
        if command.values.enabled {
            let current = self.settings().await?;
            let configured = command.password.is_some()
                || (!command.clear_password && current.password.is_some());
            command.values.validate_delivery(configured)?;
        }
        self.repository
            .update_settings(command, context)
            .await
            .map_err(store_error)
    }

    async fn status(&self) -> Result<AlertStatus, AdminError> {
        self.repository.status().await.map_err(store_error)
    }

    async fn preview(&self) -> Result<AlertSample, AdminError> {
        let settings = self.settings().await?;
        let now = Utc::now();
        self.repository
            .sample(
                now - TimeDelta::minutes(i64::from(settings.values.window_minutes)),
                now,
            )
            .await
            .map_err(store_error)
    }

    async fn test_email(
        &self,
        expected_revision: u64,
        context: &MutationContext,
    ) -> Result<(), AdminError> {
        let settings = self.settings().await?;
        if settings.revision != expected_revision {
            return Err(AdminError::conflict("邮件配置已改变，请刷新后重试"));
        }
        settings
            .values
            .validate_delivery(settings.password.is_some())?;
        self.sender.validate_addresses(&settings.values)?;
        self.repository
            .queue_test(expected_revision, context)
            .await
            .map_err(store_error)
    }
}

fn store_error(error: crate::ports::store::AdminStoreError) -> AdminError {
    map_store_error(error, "email alerts")
}
