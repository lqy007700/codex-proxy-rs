//! 启动模式共享的基础设施与 Provider 组装，不启动 HTTP 或 Worker

use gateway_admin::ports::provider::ProviderAdminRegistry;
use gateway_core::{engine::provider::ProviderRegistry, time::DeploymentTimeZone};
use gateway_host::{HostBundle, HostConfig};
use gateway_store::{StoreBundle, StoreConfig};

use super::BootstrapError;

pub(super) struct Environment {
    pub(super) host: HostBundle,
    pub(super) store: StoreBundle,
    pub(super) timezone: DeploymentTimeZone,
}

impl Environment {
    pub(super) async fn initialize(
        host: HostConfig,
        store: StoreConfig,
    ) -> Result<Self, BootstrapError> {
        let timezone = host.timezone;
        let store = store.with_timezone(timezone);
        let host = gateway_host::initialize(host).await?;
        host.report_startup_ready("Host");
        let store = gateway_store::initialize(store).await?;
        host.report_startup_ready("Store");
        Ok(Self {
            host,
            store,
            timezone,
        })
    }
}

pub(super) struct Providers {
    pub(super) openai: provider_openai::ProviderBundle,
    pub(super) xai: provider_xai::ProviderBundle,
    pub(super) core: ProviderRegistry,
    pub(super) admin: ProviderAdminRegistry,
}

impl Providers {
    pub(super) async fn initialize(
        config: provider_openai::OpenAiConfig,
        environment: &Environment,
    ) -> Result<Self, BootstrapError> {
        let ports = environment.store.provider_ports();
        let openai =
            provider_openai::initialize(config.with_timezone(environment.timezone), ports.clone())
                .await?;
        environment.host.report_startup_ready("OpenAI Provider");
        let xai = provider_xai::initialize(ports).await?;
        environment.host.report_startup_ready("xAI Provider");
        let core = ProviderRegistry::new([openai.core_provider(), xai.core_provider()])?;
        let admin = ProviderAdminRegistry::new([openai.admin_provider(), xai.admin_provider()])
            .map_err(|_| gateway_admin::model::AdminError::invalid("Provider 管理身份冲突"))?;
        Ok(Self {
            openai,
            xai,
            core,
            admin,
        })
    }
}
