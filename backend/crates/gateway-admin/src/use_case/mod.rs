//! 管理控制面的用例实现

pub mod account_groups;
pub mod accounts;
pub mod affinity;
pub mod auth;
pub mod backup;
pub mod client_distribution;
pub mod client_keys;
pub mod credentials;
pub mod import_tasks;
pub mod key_usage;
pub mod observability;
pub mod proxies;
pub mod settings;
pub mod system;
pub mod usage_data;

mod credential_mutation;
mod error;
mod publication;

use credential_mutation::{
    commit_authorization, commit_credential_refresh, commit_credential_rotation,
    delete_credentials, import_proxy_binding, pending_authorization,
    publish_credentials_and_observe_quota, required_credential, validate_authorization_commit,
    validate_prepared_import, validate_prepared_rotation,
};
use error::map_provider_error;
pub(crate) use error::map_store_error;
use publication::publish_committed;
