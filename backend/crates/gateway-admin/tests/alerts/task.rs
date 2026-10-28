//! 邮件等待、失败和取消与告警评估的任务隔离测试

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use gateway_admin::{
    alerts::DefaultAlertService,
    model::{AdminError, MutationContext, alerts::*},
    ports::{
        alerts::{AlertRepository, EmailSender},
        store::{AdminStoreError, AdminStoreErrorKind, AdminStoreResult},
    },
};
use gateway_core::{
    lifecycle::CancellationToken,
    task::{
        ScheduledTask, WorkerContribution, WorkerCycleContext, WorkerId, WorkerKind, WorkerRunnable,
    },
};
use tokio::sync::Notify;

struct MemoryRepository {
    enabled: bool,
    runtime: Mutex<AlertRuntime>,
    samples: AtomicUsize,
    claimed: AtomicBool,
    completed: AtomicUsize,
}

impl MemoryRepository {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            runtime: Mutex::new(AlertRuntime::default()),
            samples: AtomicUsize::new(0),
            claimed: AtomicBool::new(false),
            completed: AtomicUsize::new(0),
        }
    }
    fn config(&self) -> AlertSettings {
        AlertSettings {
            revision: 1,
            values: AlertSettingsValues {
                enabled: self.enabled,
                consecutive_evaluations: 1,
                ..AlertSettingsValues::default()
            },
            password: None,
            updated_at: Utc::now(),
        }
    }
}

fn unused() -> AdminStoreError {
    AdminStoreError::new(AdminStoreErrorKind::Unavailable, "alerts", "unused")
}

#[async_trait]
impl AlertRepository for MemoryRepository {
    async fn settings(&self) -> AdminStoreResult<AlertSettings> {
        Ok(self.config())
    }
    async fn update_settings(
        &self,
        _: UpdateAlertSettings,
        _: &MutationContext,
    ) -> AdminStoreResult<AlertSettings> {
        Err(unused())
    }
    async fn runtime(&self) -> AdminStoreResult<AlertRuntime> {
        Ok(self.runtime.lock().unwrap().clone())
    }
    async fn status(&self) -> AdminStoreResult<AlertStatus> {
        Err(unused())
    }
    async fn sample(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> AdminStoreResult<AlertSample> {
        self.samples.fetch_add(1, Ordering::SeqCst);
        Ok(AlertSample {
            window_start: start,
            window_end: end,
            requests: 100,
            failures: 30,
        })
    }
    async fn commit_evaluation(
        &self,
        _: u64,
        generation: u64,
        evaluation: AlertEvaluation,
        _: String,
        _: String,
    ) -> AdminStoreResult<bool> {
        let mut current = self.runtime.lock().unwrap();
        if current.generation != generation {
            return Ok(false);
        }
        *current = evaluation.runtime;
        Ok(true)
    }
    async fn queue_test(&self, _: u64, _: &MutationContext) -> AdminStoreResult<()> {
        Err(unused())
    }
    async fn claim_delivery(&self) -> AdminStoreResult<Option<AlertDelivery>> {
        if self.claimed.swap(true, Ordering::SeqCst) {
            return Ok(None);
        }
        Ok(Some(AlertDelivery {
            id: "mail-fixture".to_owned(),
            attempt: 1,
            settings: self.config(),
            recipient: "ops@example.test".to_owned(),
            subject: "Fixture".to_owned(),
            body: "Fixture".to_owned(),
        }))
    }
    async fn complete_delivery(&self, _: &str, _: u32, _: bool) -> AdminStoreResult<()> {
        self.completed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct Sender {
    started: Notify,
    fail: bool,
}
#[async_trait]
impl EmailSender for Sender {
    fn validate_addresses(&self, _: &AlertSettingsValues) -> Result<(), AdminError> {
        Ok(())
    }
    async fn send(&self, _: &AlertDelivery) -> Result<(), EmailSendError> {
        self.started.notify_one();
        if self.fail {
            return Err(EmailSendError);
        }
        std::future::pending().await
    }
}

fn tasks(service: &Arc<DefaultAlertService>) -> (Box<dyn ScheduledTask>, Box<dyn ScheduledTask>) {
    let mut evaluation = None;
    let mut delivery = None;
    for contribution in service.workers().unwrap() {
        if let WorkerContribution::Registration(registration) = contribution {
            let WorkerRunnable::Scheduled { task, lease, .. } = registration.runnable else {
                panic!("scheduled task required")
            };
            assert!(lease.is_none());
            match registration.id.kind() {
                WorkerKind::AlertEvaluation => evaluation = Some(task),
                WorkerKind::AlertDelivery => delivery = Some(task),
                _ => panic!("unexpected task"),
            }
        }
    }
    (evaluation.unwrap(), delivery.unwrap())
}

fn context(kind: WorkerKind, cancellation: CancellationToken) -> WorkerCycleContext {
    WorkerCycleContext::new(WorkerId::try_new(kind, "test").unwrap(), None, cancellation)
}

#[tokio::test]
async fn pending_smtp_does_not_block_evaluation_and_cancels_with_its_worker() {
    let repository = Arc::new(MemoryRepository::new(true));
    let sender = Arc::new(Sender {
        started: Notify::new(),
        fail: false,
    });
    let service = Arc::new(DefaultAlertService::new(repository.clone(), sender.clone()));
    let (evaluation, delivery) = tasks(&service);
    let cancelled = CancellationToken::new();
    let send = delivery.run_cycle(context(WorkerKind::AlertDelivery, cancelled.clone()));
    tokio::pin!(send);
    tokio::select! {
        _ = sender.started.notified() => (),
        result = &mut send => panic!("SMTP should remain pending: {result:?}"),
    }
    tokio::time::timeout(
        Duration::from_secs(1),
        evaluation.run_cycle(context(
            WorkerKind::AlertEvaluation,
            CancellationToken::new(),
        )),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(repository.runtime.lock().unwrap().phase, AlertPhase::Firing);
    assert_eq!(repository.completed.load(Ordering::SeqCst), 0);
    cancelled.cancel();
    tokio::time::timeout(Duration::from_secs(1), send)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(repository.completed.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn disabled_alerts_do_not_sample_requests() {
    let repository = Arc::new(MemoryRepository::new(false));
    let sender = Arc::new(Sender {
        started: Notify::new(),
        fail: true,
    });
    let service = Arc::new(DefaultAlertService::new(repository.clone(), sender));
    let (evaluation, _) = tasks(&service);
    evaluation
        .run_cycle(context(
            WorkerKind::AlertEvaluation,
            CancellationToken::new(),
        ))
        .await
        .unwrap();
    assert_eq!(repository.samples.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn smtp_failure_is_recorded_without_changing_evaluation_state() {
    let repository = Arc::new(MemoryRepository::new(true));
    let sender = Arc::new(Sender {
        started: Notify::new(),
        fail: true,
    });
    let service = Arc::new(DefaultAlertService::new(repository.clone(), sender));
    let (_, delivery) = tasks(&service);
    assert!(
        delivery
            .run_cycle(context(WorkerKind::AlertDelivery, CancellationToken::new()))
            .await
            .is_err()
    );
    assert_eq!(repository.completed.load(Ordering::SeqCst), 1);
    assert_eq!(repository.runtime.lock().unwrap().phase, AlertPhase::Normal);
}

#[test]
fn quota_evaluation_has_its_own_ten_minute_schedule() {
    let repository = Arc::new(MemoryRepository::new(false));
    let sender = Arc::new(Sender {
        started: Notify::new(),
        fail: false,
    });
    let providers = gateway_admin::ports::provider::ProviderAdminRegistry::new([]).unwrap();
    let service = Arc::new(DefaultAlertService::new(repository, sender).with_providers(providers));
    let mut intervals = Vec::new();
    for contribution in service.workers().unwrap() {
        if let WorkerContribution::Registration(registration) = contribution
            && let WorkerRunnable::Scheduled { schedule, .. } = registration.runnable
        {
            intervals.push(schedule.interval().as_secs());
        }
    }
    intervals.sort_unstable();
    assert_eq!(intervals, vec![5, 60, 600]);
}
