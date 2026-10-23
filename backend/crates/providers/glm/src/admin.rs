//! 智谱账号导入、密钥轮换、安全详情与模型目录

use crate::credential::Credential;
use async_trait::async_trait;
use chrono::Utc;
use gateway_admin::{
    model::{
        Revision,
        accounts::CredentialState,
        observability::{CalculatedBillingBreakdown, DashboardWireProfile, ProviderBillingInput},
        provider_credentials::*,
    },
    ports::provider::{ProviderAdmin, ProviderAdminError, ProviderAdminErrorKind},
};
use gateway_core::{
    account::{OpaqueProviderData, ProviderAccountId, ProviderAccountStore},
    operation::{GenerateRequest, Operation, ProtocolPayload},
    routing::{ProviderKind, UpstreamModelId},
};
use serde_json::{Map, Value};
use std::sync::Arc;

/// 智谱凭据与模型管理的 Provider 端口实现
pub struct GlmAdmin {
    kind: ProviderKind,
    accounts: Arc<dyn ProviderAccountStore>,
}

pub(crate) fn invalid() -> ProviderAdminError {
    ProviderAdminError::new(ProviderAdminErrorKind::Invalid)
        .with_public_message("请提供有效的智谱标准 API Key 和 Responses API 前缀")
}
fn unsupported() -> ProviderAdminError {
    ProviderAdminError::new(ProviderAdminErrorKind::Unsupported)
}
fn document(value: Map<String, Value>) -> ProviderDocument {
    ProviderDocument::new(OpaqueProviderData::new(value))
}
struct Commit;
impl CredentialCommitGuard for Commit {
    fn finish(self: Box<Self>) {}
}

impl GlmAdmin {
    /// 组合智谱管理操作所需的通用账号存储
    pub fn new(kind: ProviderKind, accounts: Arc<dyn ProviderAccountStore>) -> Self {
        Self { kind, accounts }
    }

    async fn load(
        &self,
        id: &ProviderAccountId,
    ) -> Result<gateway_core::account::LoadedCredential, ProviderAdminError> {
        let loaded = self
            .accounts
            .load_current_credential(id)
            .await
            .map_err(|_| ProviderAdminError::new(ProviderAdminErrorKind::Unavailable))?;
        if loaded.account.provider() != &self.kind
            || loaded.account.authentication_kind() != "api_key"
        {
            return Err(invalid());
        }
        Ok(loaded)
    }
}

#[async_trait]
impl ProviderAdmin for GlmAdmin {
    fn provider_kind(&self) -> &ProviderKind {
        &self.kind
    }
    async fn account_unavailable(&self, _: &ProviderAccountId) {}
    fn dashboard_wire_profile(&self) -> Option<DashboardWireProfile> {
        None
    }
    fn calculated_billing(
        &self,
        _: &ProviderBillingInput,
    ) -> Result<Option<CalculatedBillingBreakdown>, ProviderAdminError> {
        Ok(None)
    }

    fn evaluate_usage_pricing(
        &self,
        input: &gateway_admin::ports::provider::ProviderUsagePricingInput,
        pricing: Option<&gateway_core::metering::ModelPriceOverride>,
    ) -> Result<Option<gateway_core::metering::CalculatedCostBreakdown>, ProviderAdminError> {
        Ok(crate::pricing::breakdown(&input.usage, pricing))
    }

    async fn connection_test_operation(
        &self,
        model: &UpstreamModelId,
        text: &str,
    ) -> Result<Operation, ProviderAdminError> {
        let payload = ProtocolPayload::json_object("openai", serde_json::json!({"model":model.as_str(),"input":text,"stream":true,"store":false,"max_output_tokens":64,"reasoning":{"effort":"none"}}).as_object().cloned().ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
        Ok(Operation::Generate(GenerateRequest::from_protocol_payload(
            payload,
        )))
    }

    async fn prepare_import(
        &self,
        command: PrepareCredentialImport,
    ) -> Result<PreparedCredentialImport, ProviderAdminError> {
        let mut data = command.document.into_provider_data().into_inner();
        let entries = match data.remove("accounts") {
            Some(Value::Array(entries)) if !entries.is_empty() && entries.len() <= 128 => entries,
            Some(_) => return Err(invalid()),
            None => vec![Value::Object(data)],
        };
        let mut credentials = Vec::with_capacity(entries.len());
        for entry in entries {
            let mut entry = entry.as_object().cloned().ok_or_else(invalid)?;
            if entry.remove("provider").is_some_and(|value| value != "glm")
                || entry
                    .remove("authentication_kind")
                    .is_some_and(|value| value != "api_key")
            {
                return Err(invalid());
            }
            let name = entry
                .remove("name")
                .and_then(|v| v.as_str().map(str::to_owned))
                .filter(|v| {
                    !v.trim().is_empty() && v.len() <= 256 && !v.chars().any(char::is_control)
                })
                .ok_or_else(invalid)?;
            let id = ProviderAccountId::new(format!("acct_{}", uuid::Uuid::new_v4().simple()))
                .map_err(|_| invalid())?;
            let credential = Credential::parse(&entry).map_err(|_| invalid())?;
            credentials.push(PreparedCredentialCreate {
                model_access: None,
                outbound_proxy: command.default_outbound_proxy.clone(),
                account_id: id,
                provider_kind: self.kind.clone(),
                name,
                email: None,
                upstream_user_id: None,
                upstream_account_id: None,
                plan_type: Some("payg".to_owned()),
                authentication_kind: "api_key".to_owned(),
                provider_material: document(credential.document().map_err(|_| invalid())?),
                has_refresh_token: false,
                access_token_expires_at: None,
                next_refresh_at: None,
                enabled: true,
                credential_state: CredentialState::Ready,
                credential_observed_at: Utc::now(),
            });
        }
        Ok(PreparedCredentialImport {
            provider_kind: self.kind.clone(),
            credentials,
        })
    }

    async fn start_authorization(
        &self,
        _: PendingAuthorizationMutation,
    ) -> Result<AuthorizationStarted, ProviderAdminError> {
        Err(unsupported())
    }
    async fn complete_authorization(
        &self,
        _: CompleteAuthorization,
    ) -> Result<PreparedAuthorizationCommit, ProviderAdminError> {
        Err(unsupported())
    }
    async fn prepare_refresh(
        &self,
        _: PrepareCredentialRefresh,
    ) -> Result<PreparedCredentialRotation, ProviderAdminError> {
        Err(unsupported())
    }

    async fn prepare_rotation(
        &self,
        command: PrepareCredentialRotation,
    ) -> Result<PreparedCredentialRotation, ProviderAdminError> {
        if command.account.provider_kind != self.kind
            || command.account.authentication_kind != "api_key"
        {
            return Err(invalid());
        }
        let id = ProviderAccountId::new(command.account.id.clone()).map_err(|_| invalid())?;
        let loaded = self.load(&id).await?;
        let mut credential =
            Credential::parse(loaded.credential.expose_to_provider()).map_err(|_| invalid())?;
        let changes = command.provider_material.into_provider_data().into_inner();
        if changes.keys().any(|key| {
            !matches!(
                key.as_str(),
                "base_url" | "api_key" | "transport" | "models"
            )
        }) {
            return Err(invalid());
        }
        let rotates_key = changes.contains_key("api_key");
        let mut merged = credential.document().map_err(|_| invalid())?;
        merged.extend(changes);
        credential = Credential::parse(&merged).map_err(|_| invalid())?;
        Ok(PreparedCredentialRotation::new(
            PreparedCredentialRotationFacts {
                account_id: id,
                provider_kind: self.kind.clone(),
                expected_credential_revision: Revision::new(loaded.account.revision().get())
                    .map_err(|_| invalid())?,
                replacement_identity: None,
                name: loaded.account.name().to_owned(),
                email: None,
                plan_type: Some("payg".to_owned()),
                preserve_profile: true,
                preserve_credential_state: !rotates_key,
                provider_material: document(credential.document().map_err(|_| invalid())?),
                has_refresh_token: false,
                access_token_expires_at: None,
                next_refresh_at: None,
            },
            Box::new(Commit),
        ))
    }

    async fn account_configuration(
        &self,
        id: &ProviderAccountId,
    ) -> Result<Option<ProviderDocument>, ProviderAdminError> {
        let loaded = self.load(id).await?;
        let credential =
            Credential::parse(loaded.credential.expose_to_provider()).map_err(|_| invalid())?;
        Ok(Some(document(credential.configuration())))
    }

    async fn quota(&self, _: ProviderQuotaRequest) -> Result<ProviderQuota, ProviderAdminError> {
        Ok(ProviderQuota {
            plan_type: None,
            observed_at: None,
            refresh_token_expires_at: None,
            windows: vec![],
            credits: None,
            limit_reached: false,
            provider_data: None,
        })
    }

    async fn models(
        &self,
        id: &ProviderAccountId,
        _: bool,
    ) -> Result<ProviderModels, ProviderAdminError> {
        let loaded = self.load(id).await?;
        let credential =
            Credential::parse(loaded.credential.expose_to_provider()).map_err(|_| invalid())?;
        Ok(ProviderModels {
            models: credential
                .models
                .iter()
                .map(|id| {
                    Ok(ProviderModel {
                        id: UpstreamModelId::new(id.clone()).map_err(|_| invalid())?,
                        name: id.clone(),
                    })
                })
                .collect::<Result<_, ProviderAdminError>>()?,
            observed_at: None,
        })
    }

    async fn export_credentials(
        &self,
        inputs: Vec<ProviderExportCredentialInput>,
    ) -> Result<ProviderExport, ProviderAdminError> {
        let mut entries = vec![];
        let mut ids = vec![];
        for input in inputs {
            if input.account.provider_kind != self.kind
                || input.account.authentication_kind != "api_key"
            {
                return Err(invalid());
            }
            let credential = Credential::parse(
                input
                    .provider_material
                    .expose_to_provider()
                    .expose_to_provider(),
            )
            .map_err(|_| invalid())?;
            let mut entry = credential.document().map_err(|_| invalid())?;
            let id = ProviderAccountId::new(input.account.id.clone()).map_err(|_| invalid())?;
            entry.insert("name".into(), Value::String(input.account.name));
            entries.push(Value::Object(entry));
            ids.push(id);
        }
        Ok(ProviderExport {
            provider_kind: self.kind.clone(),
            account_ids: ids,
            document: document(Map::from_iter([("accounts".into(), Value::Array(entries))])),
        })
    }
}
