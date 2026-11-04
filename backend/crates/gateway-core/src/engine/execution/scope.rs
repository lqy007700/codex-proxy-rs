//! 请求授权与准入释放 guard

use super::admission::{AdmissionLease, settle_budget};
use super::contract::{AuthenticatedClient, ExecutionRequestMetadata};
use crate::engine::budget::{ClientBudgetCharge, ClientBudgetPort};
use crate::{
    concurrency::ConcurrencyWaitBudget,
    engine::{
        ModelRequestId, coordinator::CoordinationExtensions, effects::ExecutionEffects,
        extensions::ExtensionCallScope,
    },
    identity::ProviderKind,
    lifecycle::{CancellationToken, Deadline},
    operation::Operation,
    policy::ClientApiKeyId,
    routing::{FrozenAccountScope, PublicModelId, UpstreamModelId},
};
use std::{sync::Arc, time::SystemTime};

pub(super) enum ExecutionTarget {
    Model(PublicModelId),
    ProviderEndpoint {
        provider: ProviderKind,
        upstream_model: Option<UpstreamModelId>,
    },
}

impl ExecutionTarget {
    pub(super) fn public_model(&self) -> Option<&PublicModelId> {
        match self {
            Self::Model(model) => Some(model),
            Self::ProviderEndpoint { .. } => None,
        }
    }

    pub(super) fn into_public_model(self) -> Option<PublicModelId> {
        match self {
            Self::Model(model) => Some(model),
            Self::ProviderEndpoint { .. } => None,
        }
    }
}

pub(super) struct PendingStartExecution {
    pub(super) client: AuthenticatedClient,
    pub(super) target: ExecutionTarget,
    pub(super) operation: Operation,
    pub(super) metadata: ExecutionRequestMetadata,
}

pub(super) struct AuthorizedExecution {
    pub(super) response_control: Option<crate::engine::response_control::ResponseControl>,
    pub(super) account_scope: Arc<FrozenAccountScope>,
    pub(super) deadline_at: Deadline,
    pub(super) cancellation: CancellationToken,
    pub(super) extension_scope: ExtensionCallScope,
    pub(super) execution_effects: Option<Arc<ExecutionEffects>>,
    pub(super) execution_effects_baseline: usize,
}

pub(super) struct ExecutionStartGuard {
    pub(super) admission: ExecutionAdmission,
    pub(super) concurrency_wait_budget: ConcurrencyWaitBudget,
    pub(super) admission_decision_ms: Option<u64>,
}

pub(super) struct PreparedExecutionStart {
    pub(super) request_id: ModelRequestId,
    pub(super) started_at: SystemTime,
    pub(super) plan: crate::routing::RoutingPlan,
    pub(super) extensions: CoordinationExtensions,
    pub(super) authorization: AuthorizedExecution,
    pub(super) guard: ExecutionStartGuard,
}

impl ExecutionStartGuard {
    pub(super) async fn release_failed(
        self,
        budget: Option<&dyn ClientBudgetPort>,
        request_id: ModelRequestId,
        key_id: ClientApiKeyId,
        diagnostics: &dyn crate::diagnostics::OperationalDiagnostics,
    ) {
        if let Some(budget) = budget {
            settle_budget(
                budget,
                ClientBudgetCharge {
                    key_id,
                    request_id,
                    amount_usd: crate::metering::Decimal::ZERO,
                    completed_at: SystemTime::now(),
                },
                diagnostics,
            )
            .await;
        }
        self.admission.release().await;
    }
}

pub(super) type ExecutionAdmission = AdmissionLease;
