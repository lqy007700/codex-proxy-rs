//! Provider 账号领域、持久化端口与同一 target 内的账号选择

mod error;
mod error_protection;
pub use error_protection::*;
mod fast_mode;
pub use fast_mode::FastMode;
mod location;
pub use location::{InvalidRequestLocation, RequestLocation};
mod model;
mod model_access;
mod model_mappings;
pub use model_access::{
    AccountModelAccess, AccountModelAccessMode, InvalidAccountModelAccess,
    MAX_ACCOUNT_ACCESS_MODELS,
};
pub use model_mappings::{AccountModelMappings, InvalidAccountModelMappings};
mod quota_limits;
pub use quota_limits::{AccountQuotaLimits, QuotaMergeBase, QuotaPauseDecision};
mod proxy;
pub use proxy::{InvalidOutboundProxy, OutboundProxy};
pub mod scope;
mod selection;
mod smart_scheduling;
mod store;
mod upstream;
pub use upstream::UpstreamConnection;

pub use error::CredentialError;
pub use model::*;
pub(crate) use selection::smart_score;
pub use selection::*;
pub use smart_scheduling::{QuotaResetPeriod, SmartSchedulingConfig, SmartSchedulingConfigError};
pub use store::{AccountCatalogObservation, ProviderAccountStore, StoredAccountCatalog};
