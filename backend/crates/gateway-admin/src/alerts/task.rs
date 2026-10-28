//! 告警评估与邮件投递的独立可取消周期任务

use std::sync::Arc;

use futures::future::BoxFuture;
use gateway_core::task::{ScheduledTask, WorkerCycleContext, WorkerTaskError};

use super::DefaultAlertService;

pub(super) enum AlertTaskKind {
    Evaluate,
    Deliver,
    Quota,
}

pub(super) struct AlertTask {
    pub service: Arc<DefaultAlertService>,
    pub kind: AlertTaskKind,
}

impl ScheduledTask for AlertTask {
    fn run_cycle(&self, context: WorkerCycleContext) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            tokio::select! {
                biased;
                _ = context.cancellation().cancelled() => Ok(()),
                result = async {
                    match self.kind {
                        AlertTaskKind::Quota => self.service.evaluate_quota_cycle().await,
                        AlertTaskKind::Evaluate => self.service.evaluate_cycle().await,
                        AlertTaskKind::Deliver => self.service.deliver_cycle().await,
                    }
                } => result.map_err(|error| WorkerTaskError::safe(error.message()).with_source(error)),
            }
        })
    }
}
