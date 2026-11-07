//! 亲和管理的会话投影、操作快照和结果合同

use chrono::{DateTime, Utc};
use gateway_core::{policy::CallerIdentity, provider_ports::SessionBindingDirective};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AffinitySession {
    pub reference: String,
    pub provider: String,
    pub observation: gateway_core::provider_ports::ProviderSessionObservation,
    pub account_id: String,
    pub revision: String,
    pub directive: Option<SessionBindingDirective>,
    pub first_observed_at: DateTime<Utc>,
    pub last_admission_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    /// 请求观测后短时间内仍在运行窗口中的会话标记
    #[serde(default)]
    pub active: bool,
}

impl AffinitySession {
    #[must_use]
    pub fn caller(&self) -> &CallerIdentity {
        &self.observation.caller
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AffinityQuery {
    pub binding_scope: Option<gateway_core::provider_ports::ProviderAffinityScope>,
    pub cursor: Option<String>,
    pub mode: Option<String>,
    pub source_id: Option<String>,
    pub client_key_id: Option<String>,
    pub user_id: Option<String>,
    pub token_id: Option<String>,
    pub provider: Option<String>,
    pub account_id: Option<String>,
    pub reference: Option<String>,
    pub request_id: Option<String>,
    pub status: Option<String>,
}

impl AffinityQuery {
    #[must_use]
    pub fn matches(&self, session: &AffinitySession) -> bool {
        let (mode, source, user, token) = match session.caller() {
            CallerIdentity::Direct { .. } => ("direct", None, None, None),
            CallerIdentity::NewApi {
                source_id,
                user_id,
                token_id,
            } => ("newApi", Some(source_id), Some(user_id), Some(token_id)),
            CallerIdentity::Unidentified { .. } => return false,
        };
        let equal = |filter: &Option<String>, value: Option<&str>| {
            filter.as_deref().is_none_or(|filter| value == Some(filter))
        };
        self.binding_scope
            .is_none_or(|scope| scope == session.observation.binding_scope)
            && equal(&self.mode, Some(mode))
            && equal(&self.source_id, source.map(String::as_str))
            && equal(&self.user_id, user.map(String::as_str))
            && equal(&self.token_id, token.map(String::as_str))
            && equal(
                &self.client_key_id,
                Some(&session.observation.client_key_id),
            )
            && equal(&self.provider, Some(&session.provider))
            && equal(&self.account_id, Some(&session.account_id))
            && equal(&self.reference, Some(&session.reference))
            && equal(&self.request_id, Some(&session.observation.request_id))
            && self.status.as_deref().is_none_or(|status| match status {
                "pending" => session.directive.is_some(),
                "bound" => session.directive.is_none(),
                "primary" | "spillover" => {
                    session.observation.binding_scope
                        == gateway_core::provider_ports::ProviderAffinityScope::Token
                        && session.directive.is_none()
                        && (session.account_id == session.observation.actual_account_id)
                            == (status == "primary")
                }
                _ => false,
            })
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AffinityPage {
    pub items: Vec<AffinitySession>,
    pub next_cursor: Option<String>,
    pub coverage: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IdentitySource {
    pub id: String,
    pub name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AffinitySelection {
    pub provider: String,
    pub reference: String,
    pub revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AffinityPreviewRequest {
    pub operation_id: String,
    pub selection: Vec<AffinitySelection>,
    pub directive: SessionBindingDirective,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AffinityPlanItem {
    pub session: AffinitySession,
    pub next_revision: String,
    pub rejection: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AffinityPlan {
    pub operation_id: String,
    pub directive: SessionBindingDirective,
    pub items: Vec<AffinityPlanItem>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AffinityItemResult {
    pub provider: String,
    pub reference: String,
    pub status: String,
    pub message: String,
    pub audit_recorded: bool,
}

impl AffinityItemResult {
    #[must_use]
    pub fn key(&self) -> String {
        format!("{}:{}", self.provider, self.reference)
    }
}

impl AffinityPlanItem {
    #[must_use]
    pub fn key(&self) -> String {
        format!("{}:{}", self.session.provider, self.session.reference)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AffinityOperation {
    pub plan: AffinityPlan,
    pub results: BTreeMap<String, AffinityItemResult>,
}
