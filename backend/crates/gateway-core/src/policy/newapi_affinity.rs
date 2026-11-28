//! New API Token 持久配置与有效请求级规则

use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

use crate::account::ProviderAccountId;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewApiTokenIdentity {
    pub source_id: String,
    pub user_id: String,
    pub token_id: String,
}

impl NewApiTokenIdentity {
    #[must_use]
    pub fn is_valid(&self) -> bool {
        uuid::Uuid::parse_str(&self.source_id).is_ok()
            && super::caller::valid_id(&self.user_id)
            && super::caller::valid_id(&self.token_id)
    }

    #[must_use]
    pub fn from_caller(caller: &super::CallerIdentity) -> Option<Self> {
        match caller {
            super::CallerIdentity::NewApi {
                source_id,
                user_id,
                token_id,
            } => Some(Self {
                source_id: source_id.clone(),
                user_id: user_id.clone(),
                token_id: token_id.clone(),
            }),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewApiTokenAffinityConfig {
    pub enabled: bool,
    pub request_limit: u32,
    pub target_account: Option<String>,
}

impl Default for NewApiTokenAffinityConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            request_limit: 50,
            target_account: None,
        }
    }
}

impl NewApiTokenAffinityConfig {
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.request_limit > 0
            && (!self.enabled || self.target_account.is_some())
            && self
                .target_account
                .as_ref()
                .is_none_or(|id| ProviderAccountId::new(id.clone()).is_ok())
    }

    #[must_use]
    pub fn effective_rule(&self) -> Option<NewApiTokenAffinityRule> {
        if !self.enabled {
            return None;
        }
        Some(NewApiTokenAffinityRule {
            request_limit: NonZeroU32::new(self.request_limit)?,
            target_account: ProviderAccountId::new(self.target_account.clone()?).ok()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewApiTokenAffinityRule {
    pub request_limit: NonZeroU32,
    pub target_account: ProviderAccountId,
}
