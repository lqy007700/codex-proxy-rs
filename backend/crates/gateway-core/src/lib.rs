//! 多平台 AI 网关的数据面核心
//!
//! 本 crate 只描述协议与 Provider 无关的业务语义
//! HTTP、数据库、Redis、
//! 具体客户端协议和具体 Provider 都通过外层 adapter 接入

pub mod account;
pub mod concurrency;
pub mod diagnostics;
pub mod engine;
pub mod error;
pub mod event;
pub mod health;
pub mod identity;
pub mod lifecycle;
pub mod live;
pub mod metering;
pub mod middleware;
pub mod operation;
pub mod policy;
pub mod provider_ports;
pub mod routing;
pub mod runtime;
pub mod settings;
pub mod task;
pub mod time;
pub mod upstream;
pub mod validation;

use std::sync::Arc;
use std::time::SystemTime;

use engine::ExecutionStore;
use engine::admission::{
    ClientAdmissionPort, ClientAdmissionRecoveryPort, restore_client_admission_startup,
};
use engine::continuation::NativeContinuationPort;
use engine::execution::{
    ClientApiKeyUsageSink, ClientKeyVerifier, DefaultExecutionService, ExecutionService,
};
use engine::probe::AccountProbe;
use engine::provider::ProviderRegistry;
use health::HealthProbe;
use routing::snapshot::{RuntimeSnapshotCompiler, SnapshotStorePort};
use runtime::{
    RuntimeSnapshotHandle, RuntimeSnapshotPublisher, SnapshotControl, SnapshotSubscriptionPort,
};
use task::{WorkerContribution, WorkerDefinitionError};

/// Store 提供给数据面 Core 的封闭能力集合
#[derive(Clone)]
pub struct CoreStorePorts {
    execution: Arc<dyn ExecutionStore>,
    diagnostics: Arc<dyn diagnostics::OperationalDiagnostics>,
    admissions: Arc<dyn ClientAdmissionPort>,
    admission_recovery: Arc<dyn ClientAdmissionRecoveryPort>,
    continuation: Arc<dyn NativeContinuationPort>,
    snapshots: Arc<dyn SnapshotStorePort>,
    snapshot_subscriptions: Arc<dyn SnapshotSubscriptionPort>,
    client_api_key_usage: Arc<dyn ClientApiKeyUsageSink>,
    budget: Option<Arc<dyn engine::budget::ClientBudgetPort>>,
    client_retry: Option<Arc<dyn engine::client_retry::ClientRetryStore>>,
    account_error_protection: Option<Arc<dyn crate::provider_ports::ProviderCooldownPort>>,
}

impl CoreStorePorts {
    #[must_use]
    pub fn new(
        execution: Arc<dyn ExecutionStore>,
        (admissions, admission_recovery): (
            Arc<dyn ClientAdmissionPort>,
            Arc<dyn ClientAdmissionRecoveryPort>,
        ),
        continuation: Arc<dyn NativeContinuationPort>,
        (snapshots, snapshot_subscriptions): (
            Arc<dyn SnapshotStorePort>,
            Arc<dyn SnapshotSubscriptionPort>,
        ),
        client_api_key_usage: Arc<dyn ClientApiKeyUsageSink>,
        diagnostics: Arc<dyn diagnostics::OperationalDiagnostics>,
    ) -> Self {
        Self {
            execution,
            diagnostics,
            admissions,
            admission_recovery,
            continuation,
            snapshots,
            snapshot_subscriptions,
            client_api_key_usage,
            budget: None,
            client_retry: None,
            account_error_protection: None,
        }
    }

    #[must_use]
    pub fn with_budget(mut self, budget: Arc<dyn engine::budget::ClientBudgetPort>) -> Self {
        self.budget = Some(budget);
        self
    }

    #[must_use]
    pub fn with_account_error_protection(
        mut self,
        store: Arc<dyn crate::provider_ports::ProviderCooldownPort>,
    ) -> Self {
        self.account_error_protection = Some(store);
        self
    }

    #[must_use]
    pub fn with_client_retry_store(
        mut self,
        store: Arc<dyn engine::client_retry::ClientRetryStore>,
    ) -> Self {
        self.client_retry = Some(store);
        self
    }
}

pub struct CoreBundle {
    snapshots: RuntimeSnapshotHandle,
    execution: Arc<dyn ExecutionService>,
    client_key_verifier: Arc<dyn ClientKeyVerifier>,
    snapshot_control: Arc<dyn SnapshotControl>,
    account_probe: Arc<dyn AccountProbe>,
    health_probes: Vec<Arc<dyn HealthProbe>>,
    worker_contributions: Vec<WorkerContribution>,
}

/// 已构造但尚未恢复准入或发现 Provider 目录的数据面启动对象
pub struct CoreStartup {
    ports: CoreStorePorts,
    providers: ProviderRegistry,
    control: CoreControlPlaneStartup,
}

impl CoreStartup {
    #[must_use]
    pub fn snapshots(&self) -> RuntimeSnapshotHandle {
        self.control.snapshots()
    }

    #[must_use]
    pub fn snapshot_control(&self) -> Arc<dyn SnapshotControl> {
        self.control.snapshot_control()
    }

    /// 绑定组合根依赖后执行 fail-closed 准入恢复与首次目录发现
    pub async fn activate(self) -> Result<CoreBundle, CoreError> {
        restore_client_admission_startup(
            self.ports.execution.as_ref(),
            self.ports.admission_recovery.as_ref(),
            self.ports.admissions.as_ref(),
            SystemTime::now(),
        )
        .await
        .map_err(|_| CoreError::AdmissionRecoveryUnavailable)?;
        let control = self.control.activate().await?;
        let snapshots = control.snapshots;
        let publisher = control.publisher;
        let worker_contributions = publisher.worker_contributions()?;
        let service = Arc::new(build_execution_service(
            snapshots.clone(),
            &self.ports,
            self.providers,
        ));
        let execution: Arc<dyn ExecutionService> = service.clone();
        let client_key_verifier: Arc<dyn ClientKeyVerifier> = service.clone();
        let account_probe: Arc<dyn AccountProbe> = service;
        let snapshot_control: Arc<dyn SnapshotControl> = publisher;
        let health_probes: Vec<Arc<dyn HealthProbe>> = vec![Arc::new(snapshots.clone())];
        Ok(CoreBundle {
            snapshots,
            execution,
            client_key_verifier,
            snapshot_control,
            account_probe,
            health_probes,
            worker_contributions,
        })
    }
}

fn build_execution_service(
    snapshots: RuntimeSnapshotHandle,
    ports: &CoreStorePorts,
    providers: ProviderRegistry,
) -> DefaultExecutionService {
    let mut service = DefaultExecutionService::new(
        snapshots,
        Arc::clone(&ports.execution),
        providers,
        Arc::clone(&ports.admissions),
        Arc::clone(&ports.continuation),
        Arc::clone(&ports.client_api_key_usage),
        Arc::clone(&ports.diagnostics),
    );
    if let Some(budget) = &ports.budget {
        service = service.with_budget(Arc::clone(budget));
    }
    if let Some(store) = &ports.account_error_protection {
        service = service.with_account_error_protection(Arc::clone(store));
    }
    if let Some(store) = &ports.client_retry {
        service = service.with_client_retry_store(Arc::clone(store));
    }
    service
}

impl CoreBundle {
    #[must_use]
    pub fn snapshots(&self) -> RuntimeSnapshotHandle {
        self.snapshots.clone()
    }
    #[must_use]
    pub fn execution_service(&self) -> Arc<dyn ExecutionService> {
        Arc::clone(&self.execution)
    }

    #[must_use]
    pub fn client_key_verifier(&self) -> Arc<dyn ClientKeyVerifier> {
        Arc::clone(&self.client_key_verifier)
    }

    #[must_use]
    pub fn snapshot_control(&self) -> Arc<dyn SnapshotControl> {
        Arc::clone(&self.snapshot_control)
    }

    #[must_use]
    pub fn account_probe(&self) -> Arc<dyn AccountProbe> {
        Arc::clone(&self.account_probe)
    }

    #[must_use]
    pub fn health_probes(&self) -> Vec<Arc<dyn HealthProbe>> {
        self.health_probes.clone()
    }

    pub fn take_worker_contributions(&mut self) -> Vec<WorkerContribution> {
        std::mem::take(&mut self.worker_contributions)
    }
}

/// 首个快照与准入恢复均为监听前 fail-closed 屏障
pub async fn initialize(
    ports: CoreStorePorts,
    providers: ProviderRegistry,
) -> Result<CoreBundle, CoreError> {
    prepare(ports, providers).activate().await
}

/// 构造 Core 与快照发布能力；首次 Provider 目录发现延迟到 [`CoreStartup::activate`]
#[must_use]
pub fn prepare(ports: CoreStorePorts, providers: ProviderRegistry) -> CoreStartup {
    let control = prepare_control_plane(ports.clone(), providers.clone());
    CoreStartup {
        ports,
        providers,
        control,
    }
}

/// 管理命令只持有快照读写能力，不恢复数据面准入，也不启动执行/结算 Worker
pub struct CoreControlPlaneBundle {
    snapshots: RuntimeSnapshotHandle,
    publisher: Arc<RuntimeSnapshotPublisher>,
}

/// CLI/组合根可先绑定回调端口，再激活首次目录发现
pub struct CoreControlPlaneStartup {
    snapshots: RuntimeSnapshotHandle,
    publisher: Arc<RuntimeSnapshotPublisher>,
}

impl CoreControlPlaneStartup {
    #[must_use]
    pub fn snapshots(&self) -> RuntimeSnapshotHandle {
        self.snapshots.clone()
    }

    #[must_use]
    pub fn snapshot_control(&self) -> Arc<dyn SnapshotControl> {
        self.publisher.clone()
    }

    pub async fn activate(self) -> Result<CoreControlPlaneBundle, CoreError> {
        match self.publisher.refresh().await {
            Ok(_) => {}
            // 配置暂不可用时保留管理修复入口，数据面仍拒绝新请求
            Err(routing::snapshot::RuntimeSnapshotCompileError::ExtensionsUnavailable) => {}
            Err(_) => return Err(CoreError::SnapshotUnavailable),
        }
        Ok(CoreControlPlaneBundle {
            snapshots: self.snapshots,
            publisher: self.publisher,
        })
    }
}

impl CoreControlPlaneBundle {
    #[must_use]
    pub fn snapshots(&self) -> RuntimeSnapshotHandle {
        self.snapshots.clone()
    }

    #[must_use]
    pub fn snapshot_control(&self) -> Arc<dyn SnapshotControl> {
        self.publisher.clone()
    }
}

/// 仅构造管理命令所需快照能力，不恢复准入、不发现目录且不启动 Worker
#[must_use]
pub fn prepare_control_plane(
    ports: CoreStorePorts,
    providers: ProviderRegistry,
) -> CoreControlPlaneStartup {
    let compiler = RuntimeSnapshotCompiler::new(ports.snapshots, Arc::new(providers.clone()))
        .with_privacy_compiler(Arc::new(providers));
    let compiler = Arc::new(compiler);
    let snapshots = RuntimeSnapshotHandle::default();
    let mut publisher =
        RuntimeSnapshotPublisher::new(compiler, snapshots.clone(), ports.snapshot_subscriptions);
    if let Some(store) = ports.account_error_protection {
        publisher = publisher.with_account_error_protection(store);
    }
    let publisher = Arc::new(publisher);
    CoreControlPlaneStartup {
        snapshots,
        publisher,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("initial runtime snapshot is unavailable")]
    SnapshotUnavailable,
    #[error("client admission startup recovery is unavailable")]
    AdmissionRecoveryUnavailable,
    #[error(transparent)]
    WorkerDefinition(#[from] WorkerDefinitionError),
}
