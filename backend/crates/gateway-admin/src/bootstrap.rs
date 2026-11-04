//! 管理服务与后台任务的启动组装，窄端口供服务器和 CLI 复用

use std::{path::Path, sync::Arc};

use gateway_core::{engine::probe::AccountProbe, runtime::SnapshotControl};

use crate::{
    AccountsService, AdminBundle, AdminConfig, AdminRuntimePorts, AdminServices, ClientConfig,
    CredentialsService, SettingsService, UsageDataService, alerts, backup, freeze_recovery,
    model::AdminError,
    ports::{self, provider::ProviderAdminRegistry, store::AdminStorePorts},
    reset_credit_worker, use_case,
    use_case::{
        account_groups::DefaultAccountGroupService,
        accounts::DefaultAccountsService,
        auth::{AuthService, DefaultAuthService},
        backup::DefaultBackupService,
        client_distribution::DefaultClientDistributionService,
        client_keys::DefaultClientKeyService,
        observability::DefaultObservabilityService,
        settings::DefaultSettingsService,
        system::DefaultSystemService,
    },
};

/// 校验配置、接入已组装的 Provider 注册表并完成默认管理员幂等初始化
///
/// # Errors
///
/// 配置非法或默认管理员初始化失败时返回错误
pub async fn initialize(
    mut config: AdminConfig,
    mut client_config: ClientConfig,
    store: AdminStorePorts,
    runtime: AdminRuntimePorts,
) -> Result<AdminBundle, AdminError> {
    let AdminRuntimePorts {
        email_sender,
        timezone,
        pricing_source,
        providers,
        snapshot,
        account_probe: probe,
        proxy_probe,
        client_distribution,
        system,
        client_key_verifier,
    } = runtime;
    config
        .resolve_and_validate(Path::new("."))
        .map_err(|error| AdminError::invalid(error.to_string()))?;
    client_config
        .resolve_and_validate(Path::new("."))
        .map_err(|error| AdminError::invalid(error.to_string()))?;
    let affinity = store
        .affinity()
        .map(|ports| use_case::affinity::AffinityService::new(ports, snapshot.clone()));
    let auth = Arc::new(DefaultAuthService::new(
        &config,
        client_config.session_ttl_minutes,
        store.auth(),
        client_key_verifier.clone(),
    ));
    auth.ensure_default_admin(config.default_password.expose())
        .await?;

    let accounts = initialize_accounts(
        store.accounts(),
        store.account_runtime(),
        store.settings(),
        timezone,
        providers.clone(),
        snapshot.clone(),
        probe.clone(),
    );
    let backup_ports = store.backup();
    let backups = Arc::new(DefaultBackupService::new(
        backup_ports.repository(),
        backup_ports.object_store(),
        store.auth(),
        snapshot.clone(),
        timezone,
    ));
    let backup_task = backup::task::BackupTask::new(
        backup_ports.repository(),
        backup_ports.dump(),
        backup_ports.object_store(),
    )
    .with_timezone(timezone);
    let system = Arc::new(DefaultSystemService::new(system));
    let observations: Arc<dyn ports::store::ObservabilityStore> = Arc::new(
        use_case::observability::CachedObservabilityStore::new(store.observability()),
    );
    let key_usage = Arc::new(use_case::key_usage::DefaultKeyUsageService::new(
        auth.clone(),
        client_key_verifier,
        store.client_keys(),
        observations.clone(),
        system.clone(),
        timezone,
    ));
    let credentials = Arc::new(CredentialsService::new(
        providers.clone(),
        store.accounts(),
        store.proxies(),
        snapshot.clone(),
    ));
    let import_tasks = use_case::import_tasks::DefaultImportTasksService::new(credentials.clone());
    let import_task = use_case::import_tasks::ImportTaskWorker(import_tasks.clone());
    let settings = initialize_settings(
        store.settings(),
        snapshot.clone(),
        providers.clone(),
        pricing_source,
    );
    let usage_data = initialize_usage_data(store.usage_data(), store.settings(), providers.clone());
    let alerts = Arc::new(
        alerts::DefaultAlertService::new(store.alerts(), email_sender)
            .with_providers(providers.clone()),
    );
    let alert_workers = alerts
        .workers()
        .map_err(|_| AdminError::internal("告警任务注册失败"))?;
    let services = AdminServices {
        timezone,
        key_usage,
        proxies: Arc::new(use_case::proxies::DefaultProxiesService::new(
            store.proxies(),
            proxy_probe,
            snapshot.clone(),
            providers.clone(),
        )),
        auth,
        accounts: accounts.clone(),
        account_groups: Arc::new(DefaultAccountGroupService::new(
            store.account_groups(),
            store.account_runtime(),
            snapshot.clone(),
        )),
        affinity,
        client_keys: Arc::new(DefaultClientKeyService::new(
            store.client_keys(),
            snapshot.clone(),
            providers.clone(),
        )),
        client_distribution: Arc::new(DefaultClientDistributionService::new(client_distribution)),
        observability: Arc::new(DefaultObservabilityService::new(
            observations.clone(),
            store.accounts(),
            store.settings(),
            providers.clone(),
            timezone,
            system.clone(),
        )),
        settings,
        alerts,
        usage_data,
        system,
        credentials,
        import_tasks,
        backups,
    };
    let freeze_recovery =
        freeze_recovery::FreezeRecoveryTask::new(freeze_recovery::FreezeRecoveryDeps {
            accounts: Arc::clone(&accounts) as Arc<dyn AccountsService>,
            store: store.accounts(),
            runtime: store.account_runtime(),
            settings: store.settings(),
        });
    let mut worker_contributions = vec![
        backup_task.into_worker()?,
        import_task.into_worker()?,
        freeze_recovery.into_worker()?,
        reset_credit_worker::ResetCreditAutomationTask {
            accounts,
            store: store.accounts(),
            settings: store.settings(),
        }
        .into_worker()?,
    ];
    worker_contributions.extend(alert_workers);
    Ok(AdminBundle {
        services,
        worker_contributions,
    })
}

/// 服务端与 CLI 复用账号用例，不随命令启动后台调度
#[must_use]
pub fn initialize_accounts(
    accounts: Arc<dyn ports::store::AccountStore>,
    account_runtime: Arc<dyn ports::store::AccountRuntimeStore>,
    settings: Arc<dyn ports::store::SettingsStore>,
    timezone: gateway_core::time::DeploymentTimeZone,
    providers: ProviderAdminRegistry,
    snapshot: Arc<dyn SnapshotControl>,
    probe: Arc<dyn AccountProbe>,
) -> Arc<dyn AccountsService> {
    Arc::new(DefaultAccountsService::new(
        accounts,
        account_runtime,
        settings,
        timezone,
        providers,
        snapshot,
        probe,
    ))
}

/// 设置服务不依赖 Web 管理会话，CLI 与服务器通过同一用例执行事务和发布
pub fn initialize_settings(
    store: Arc<dyn ports::store::SettingsStore>,
    snapshot: Arc<dyn SnapshotControl>,
    providers: ProviderAdminRegistry,
    pricing_source: Arc<dyn ports::pricing::PricingSource>,
) -> Arc<dyn SettingsService> {
    Arc::new(DefaultSettingsService::new(
        store,
        snapshot,
        providers,
        pricing_source,
    ))
}

/// 原生用量领域用例，调用边界提供已验证的主体
#[must_use]
pub fn initialize_usage_data(
    store: Arc<dyn ports::usage_data::UsageDataStore>,
    settings: Arc<dyn ports::store::SettingsStore>,
    providers: ProviderAdminRegistry,
) -> Arc<dyn UsageDataService> {
    Arc::new(use_case::usage_data::DefaultUsageDataService::new(
        store, settings, providers,
    ))
}
