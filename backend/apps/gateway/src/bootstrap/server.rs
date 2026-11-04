//! 服务模式组装数据面、管理服务与后台任务，并交给 Host 监听

use super::{
    BootstrapError, GatewayConfig,
    startup::{Environment, Providers},
};

/// 按冻结顺序初始化全部 Bundle，并把进程阻塞权交给 Host
pub async fn run() -> Result<(), BootstrapError> {
    let config = gateway_host::load_config::<GatewayConfig>()?;
    let environment = Environment::initialize(config.host, config.store).await?;
    let mut providers = Providers::initialize(config.openai, &environment).await?;
    let Environment {
        host,
        mut store,
        timezone,
    } = environment;
    let mut core = gateway_core::prepare(store.core_ports(), providers.core)
        .activate()
        .await?;
    host.report_startup_ready("Core");
    let analytics_worker = store.request_analytics_worker(std::sync::Arc::new(
        gateway_admin::ProviderUsageBillingProjector::new(providers.admin.clone()),
    ))?;
    let mut admin = gateway_admin::initialize(
        config.admin,
        config.client,
        store.admin_ports(),
        gateway_admin::AdminRuntimePorts {
            timezone,
            providers: providers.admin,
            pricing_source: std::sync::Arc::new(gateway_host::pricing::ModelsDevPricing),
            email_sender: std::sync::Arc::new(gateway_host::email::SmtpEmailSender),
            snapshot: core.snapshot_control(),
            account_probe: core.account_probe(),
            proxy_probe: host.proxy_probe(provider_openai::build_reqwest_client_with_custom_ca),
            client_distribution: host.client_distribution_resolver(),
            system: host.system_operations(),
            client_key_verifier: core.client_key_verifier(),
        },
    )
    .await?;
    host.report_startup_ready("Admin");
    let mut probes = store.health_probes();
    probes.extend(core.health_probes());
    probes.push(host.logging_health_probe());
    let api = gateway_api::initialize(
        config.api,
        core.execution_service(),
        admin.services(),
        probes,
        host.worker_health(),
        host.connection_lifecycle(),
        store.diagnostics(),
    )?;
    host.report_startup_ready("API");
    let mut plan = store.take_worker_contributions();
    plan.push(analytics_worker);
    plan.push(gateway_host::retention::worker(store.retention())?);
    plan.extend(core.take_worker_contributions());
    plan.extend(providers.openai.take_worker_contributions());
    plan.extend(providers.xai.take_worker_contributions());
    plan.extend(admin.take_worker_contributions());
    host.start_workers(plan, store.worker_leader_lease(), store.diagnostics())?;
    host.report_startup_ready("Workers");
    Ok(host.serve(api.router()).await?)
}
