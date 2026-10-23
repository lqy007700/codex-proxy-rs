//! 智谱标准 API Key、Responses 数据面与管理能力的独立 Provider

mod admin;
pub use admin::GlmAdmin;
mod credential;
mod pricing;
mod provider;
pub use provider::GlmProvider;
mod response;

use gateway_admin::ports::provider::ProviderAdmin;
use gateway_core::{engine::provider::Provider, provider_ports::ProviderStorePorts};
use std::sync::Arc;

/// 智谱初始化后交给组合根的能力集合
pub struct ProviderBundle {
    provider: Arc<provider::GlmProvider>,
    admin: Arc<admin::GlmAdmin>,
}

/// 初始化只组合本地能力，不访问智谱或修改其它 Provider 状态
pub fn initialize(ports: ProviderStorePorts) -> Result<ProviderBundle, GlmInitializeError> {
    let kind = gateway_core::routing::ProviderKind::new("glm").map_err(|_| GlmInitializeError)?;
    let accounts = ports.accounts();
    Ok(ProviderBundle {
        provider: Arc::new(provider::GlmProvider::new(
            kind.clone(),
            ports.accounts(),
            ports.leases(),
            ports.cooldowns(),
            ports.account_feedback(),
        )),
        admin: Arc::new(admin::GlmAdmin::new(kind, accounts)),
    })
}

impl ProviderBundle {
    #[must_use]
    pub fn core_provider(&self) -> Arc<dyn Provider> {
        self.provider.clone()
    }

    #[must_use]
    pub fn admin_provider(&self) -> Arc<dyn ProviderAdmin> {
        self.admin.clone()
    }
}

/// 初始化错误不携带连接地址和认证材料
#[derive(Debug, thiserror::Error)]
#[error("GLM provider initialization failed")]
pub struct GlmInitializeError;
