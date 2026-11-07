//! 唯一执行服务的认证、准入、路由与调用编排

use super::failure::{authentication_gateway_error, gateway_error_from_engine, map_routing_error};

use super::admission::{AdmissionLease, settle_budget};
use super::contract::{
    AuthenticatedClient, ClientApiKeyUsageSink, ClientAuthenticationError, ClientKeyVerifier,
    ClientTransport, ExecutionRequestMetadata, ExecutionService, PreparedExecutionRequest,
    PreparedRootExecution, StartExecution, StartProviderExecution, StartedExecution, duration_ms,
    new_request_id,
};
use super::probe_store::{ProbeObservation, TransientExecutionStore};
use super::scope::{
    AuthorizedExecution, ExecutionStartGuard, ExecutionTarget, PendingStartExecution,
    PreparedExecutionStart,
};
use super::session::DefaultExecutionSession;
use super::{COORDINATION_TIMEOUT, DIAGNOSTIC_TIMEOUT};
use crate::{
    concurrency::{CapacityWait, ConcurrencyWaitBudget, ConcurrencyWaitQueue},
    engine::{
        AttemptCoordinator, EngineError, ExecutionStore, GatewayEngine, ModelRequestId,
        NewModelRequest, ProbeFailure, UpstreamSendState,
        admission::{
            ClientAdmissionDecision, ClientAdmissionPort, ClientAdmissionRejection,
            ClientAdmissionRequest,
        },
        authentication::ClientAuthenticationRequest,
        budget::{ClientBudgetCharge, ClientBudgetPort},
        continuation::{
            ContinuationBinding, NativeContinuationPin, NativeContinuationPort,
            NativeContinuationStoreErrorKind,
        },
        coordinator::CoordinationExtensions,
        extensions::ExecutionExtensionIndex,
        middleware::FrozenMiddlewarePlan,
        observation::{
            FrozenRequestObservationContext, RequestObservationDispatch, RequestObservationScope,
            RequestObserverExtensionIndex,
        },
        policy::{ModelRouteDecision, RequestPolicyContext, RequestPolicyExtensionIndex},
        probe::{
            AccountProbe, AccountProbeError, AccountProbeErrorSource, AccountProbeRequest,
            AccountProbeResult, AccountProbeUpstreamResponse,
        },
        provider::ProviderRegistry,
    },
    error::{GatewayError, GatewayErrorKind},
    event::GatewayEvent,
    identity::ProviderKind,
    lifecycle::{CancellationToken, Deadline, REQUEST_LEASE_TTL},
    operation::Operation,
    policy::{ClientApiKeyId, ClientPolicy},
    routing::{
        ProviderCatalogUnavailable, ProviderModelContent, PublicModelDescriptor, PublicModelId,
        RoutingContext, RuntimeSnapshot, UpstreamModelId, request_settings::RequestSettings,
    },
    runtime::RuntimeSnapshotHandle,
};
use futures::{FutureExt as _, future::BoxFuture, pin_mut, select_biased};
use futures_timer::Delay;
use std::{
    collections::BTreeSet,
    sync::Arc,
    time::{Instant, SystemTime},
};

pub struct DefaultExecutionService {
    snapshots: RuntimeSnapshotHandle,
    /// probe 自身走 transient store，探测失败仍写入持久 store 的 ops_events
    observations: Arc<dyn ExecutionStore>,
    diagnostics: Arc<dyn crate::diagnostics::OperationalDiagnostics>,
    providers: ProviderRegistry,
    admissions: Arc<dyn ClientAdmissionPort>,
    admission_waiting: ConcurrencyWaitQueue<ClientApiKeyId>,
    continuation: Arc<dyn NativeContinuationPort>,
    client_api_key_usage: Arc<dyn ClientApiKeyUsageSink>,
    budget: Option<Arc<dyn ClientBudgetPort>>,
    request_observers: Option<RequestObserverExtensionIndex>,
    request_policies: Option<RequestPolicyExtensionIndex>,
    execution_extensions: Option<ExecutionExtensionIndex>,
    client_retry: Option<Arc<dyn crate::engine::client_retry::ClientRetryStore>>,
    account_error_protection: Option<Arc<dyn crate::provider_ports::ProviderCooldownPort>>,
}

impl DefaultExecutionService {
    #[must_use]
    pub fn new(
        snapshots: RuntimeSnapshotHandle,
        execution: Arc<dyn ExecutionStore>,
        providers: ProviderRegistry,
        admissions: Arc<dyn ClientAdmissionPort>,
        continuation: Arc<dyn NativeContinuationPort>,
        client_api_key_usage: Arc<dyn ClientApiKeyUsageSink>,
        diagnostics: Arc<dyn crate::diagnostics::OperationalDiagnostics>,
    ) -> Self {
        Self {
            snapshots,
            observations: execution,
            diagnostics,
            providers,
            admissions,
            admission_waiting: ConcurrencyWaitQueue::default(),
            continuation,
            client_api_key_usage,
            budget: None,
            request_observers: None,
            request_policies: None,
            execution_extensions: None,
            client_retry: None,
            account_error_protection: None,
        }
    }

    #[must_use]
    pub fn with_budget(mut self, budget: Arc<dyn ClientBudgetPort>) -> Self {
        self.budget = Some(budget);
        self
    }

    #[must_use]
    pub fn with_account_error_protection(
        mut self,
        store: Arc<dyn crate::provider_ports::ProviderCooldownPort>,
    ) -> Self {
        self.account_error_protection = Some(store);
        self
    }

    #[must_use]
    pub fn with_client_retry_store(
        mut self,
        store: Arc<dyn crate::engine::client_retry::ClientRetryStore>,
    ) -> Self {
        self.client_retry = Some(store);
        self
    }

    #[must_use]
    pub fn with_request_observers(mut self, observers: RequestObserverExtensionIndex) -> Self {
        self.request_observers = Some(observers);
        self
    }

    #[must_use]
    pub fn with_request_policies(mut self, policies: RequestPolicyExtensionIndex) -> Self {
        self.request_policies = Some(policies);
        self
    }

    #[must_use]
    pub fn with_execution_extensions(
        mut self,
        execution_extensions: ExecutionExtensionIndex,
    ) -> Self {
        self.execution_extensions = Some(execution_extensions);
        self
    }

    fn authenticate_without_usage(
        &self,
        plaintext: &str,
    ) -> Result<AuthenticatedClient, ClientAuthenticationError> {
        let authentication = ClientAuthenticationRequest::bearer(plaintext)
            .map_err(|_| ClientAuthenticationError::InvalidKey)?;
        let snapshot = self
            .snapshots
            .acquire()
            .map_err(|_| ClientAuthenticationError::SnapshotUnavailable)?;
        let policy = snapshot
            .client_policies()
            .filter(|policy| {
                constant_time_equal(plaintext, policy.plaintext_key().expose_for_auth())
            })
            .find(|policy| policy.authorize().is_ok())
            .cloned()
            .ok_or(ClientAuthenticationError::InvalidKey)?;
        let policy = RequestSettings::new(snapshot.clone())
            .apply_policy(policy)
            .freeze_caller(authentication.forwarded_caller())
            .map_err(|_| ClientAuthenticationError::InvalidKey)?;
        Ok(AuthenticatedClient {
            settings: None,
            snapshot,
            policy,
            authentication: Some(authentication),
        })
    }

    async fn authenticate_request_without_usage(
        &self,
        mut authentication: ClientAuthenticationRequest,
    ) -> Result<AuthenticatedClient, ClientAuthenticationError> {
        let snapshot = match authentication.settings() {
            Some(settings) => settings.snapshot(),
            None => self
                .snapshots
                .acquire()
                .map_err(|_| ClientAuthenticationError::SnapshotUnavailable)?,
        };
        let policy = native_policy(&snapshot, &authentication)?;
        let policy = match authentication.settings() {
            Some(settings) => settings.apply_policy(policy),
            None => RequestSettings::new(snapshot.clone()).apply_policy(policy),
        };
        let policy = policy
            .freeze_caller(authentication.forwarded_caller())
            .map_err(|_| ClientAuthenticationError::InvalidKey)?;
        Ok(AuthenticatedClient {
            settings: authentication.take_settings(),
            snapshot,
            policy,
            authentication: Some(authentication),
        })
    }

    async fn start_inner(&self, request: StartExecution) -> Result<StartedExecution, GatewayError> {
        let StartExecution {
            client,
            public_model,
            operation,
            metadata,
        } = request;
        let prepared = self.prepare_root_execution_inner(client).await?;
        self.start_prepared_with_target(
            prepared,
            ExecutionTarget::Model(public_model),
            operation,
            metadata,
        )
        .await
    }

    async fn start_provider_endpoint_inner(
        &self,
        request: StartProviderExecution,
    ) -> Result<StartedExecution, GatewayError> {
        let StartProviderExecution {
            client,
            provider,
            upstream_model,
            operation,
            metadata,
        } = request;
        let prepared = self.prepare_root_execution_inner(client).await?;
        self.start_prepared_with_target(
            prepared,
            ExecutionTarget::ProviderEndpoint {
                provider,
                upstream_model,
            },
            operation,
            metadata,
        )
        .await
    }

    async fn prepare_root_execution_inner(
        &self,
        mut client: AuthenticatedClient,
    ) -> Result<PreparedRootExecution, GatewayError> {
        // 长连接每次执行都重新鉴权并冻结当前策略，确保限额和授权变更对新请求生效
        let expected_key_id = client.policy.key_id().clone();
        let expected_caller_policy = client.policy.identity_policy().clone();
        let expected_caller = client.policy.caller_identity();
        let authentication = client.authentication.clone().ok_or_else(|| {
            GatewayError::new(
                GatewayErrorKind::Internal,
                "Bound execution identity cannot enter the public start path.",
            )
        })?;
        let latest = self.snapshots.acquire().map_err(|_| {
            authentication_gateway_error(ClientAuthenticationError::SnapshotUnavailable)
        })?;
        let latest_policy = latest.client_policy(&expected_key_id);
        if expected_caller_policy != crate::policy::CallerIdentityPolicy::Direct
            || latest_policy.is_some_and(|policy| {
                policy.identity_policy() != &crate::policy::CallerIdentityPolicy::Direct
            })
        {
            let latest_policy = latest_policy
                .filter(|policy| policy.authorize().is_ok())
                .ok_or_else(|| {
                    authentication_gateway_error(ClientAuthenticationError::InvalidKey)
                })?;
            if latest_policy.identity_policy() != &expected_caller_policy {
                return Err(authentication_gateway_error(
                    ClientAuthenticationError::InvalidKey,
                ));
            }
        }
        let authentication = match client.settings.clone() {
            Some(settings) => authentication.with_settings(settings),
            None => authentication,
        };
        client = self
            .authenticate_request_without_usage(authentication)
            .await
            .and_then(|client| {
                (client.policy.key_id() == &expected_key_id
                    && client.policy.identity_policy() == &expected_caller_policy
                    && (expected_caller.is_unidentified()
                        || client.policy.caller_identity() == expected_caller))
                    .then_some(client)
                    .ok_or(ClientAuthenticationError::InvalidKey)
            })
            .map_err(authentication_gateway_error)?;
        self.client_api_key_usage
            .record_used(client.policy().key_id());
        PreparedRootExecution::new(client)
    }

    async fn start_prepared_with_target(
        &self,
        prepared: PreparedRootExecution,
        target: ExecutionTarget,
        operation: Operation,
        metadata: ExecutionRequestMetadata,
    ) -> Result<StartedExecution, GatewayError> {
        let PreparedRootExecution {
            extension_scope,
            client,
            response_control,
            request_id,
            started_at,
            deadline_at,
            cancellation,
            execution_effects,
            execution_effects_baseline,
        } = prepared;
        let authorization = AuthorizedExecution {
            response_control: Some(response_control),
            account_scope: Arc::clone(client.policy.account_scope()),
            deadline_at,
            cancellation,
            extension_scope,
            execution_effects: Some(execution_effects),
            execution_effects_baseline,
        };
        self.start_authorized(
            PendingStartExecution {
                client,
                target,
                operation,
                metadata,
            },
            authorization,
            started_at,
            Some(request_id),
        )
        .await
    }

    async fn start_authorized(
        &self,
        mut request: PendingStartExecution,
        mut authorization: AuthorizedExecution,
        started_at: SystemTime,
        request_id: Option<ModelRequestId>,
    ) -> Result<StartedExecution, GatewayError> {
        let request_id = request_id.map_or_else(new_request_id, Ok)?;
        let account_group_ids = authorization
            .account_scope
            .routing_snapshot()
            .groups_snapshot()
            .iter()
            .map(|group| group.id().clone())
            .collect::<Vec<_>>();
        let upstream_adapters = request
            .client
            .snapshot
            .extensions()
            .and_then(|set| self.execution_extensions.as_ref()?.upstream_adapters(set));
        let middleware = self
            .execution_extensions
            .as_ref()
            .and_then(|execution_extensions| {
                let generation = request.client.snapshot.extensions()?.clone();
                execution_extensions.middleware(&generation)
            });
        let request_policy = self.request_policies.as_ref().and_then(|policies| {
            let generation = request.client.snapshot.extensions()?.clone();
            let plan = policies.resolve(&generation)?;
            Some(
                RequestPolicyContext::new(
                    plan,
                    generation,
                    request_id.clone(),
                    request.client.policy.key_id().clone(),
                    account_group_ids.clone(),
                )
                .with_extension_scope(authorization.extension_scope.clone())
                .with_request_body_limit(
                    request
                        .client
                        .snapshot
                        .responses_max_decompressed_body_bytes(),
                )
                .with_execution_effects(Arc::clone(authorization.execution_effects.as_ref()?)),
            )
        });
        // 请求策略执行前取得 Key 准入；原生路由保持先路由、后准入的顺序
        let mut start_guard = None;
        let request_observation = self.request_observers.as_ref().and_then(|observers| {
            let generation = request.client.snapshot.extensions()?.clone();
            let plan = observers.resolve(&generation)?;
            Some(RequestObservationDispatch::new(
                plan,
                generation,
                FrozenRequestObservationContext::new(
                    request_id.clone(),
                    request.client.snapshot.revision(),
                    RequestObservationScope::new(
                        request.client.policy.key_id().clone(),
                        account_group_ids.clone(),
                    ),
                    request.operation.kind(),
                    request.target.public_model().cloned(),
                    authorization.extension_scope.clone(),
                ),
            ))
        });
        let budget_key_id = request.client.policy.key_id().clone();
        let mut entered_execution = false;
        let result = async {
            crate::engine::image_generation::check_image_generation(
                authorization.account_scope.allows_image_generation(),
                &request.operation,
            )?;
            if authorization.cancellation.is_cancelled() {
                return Err(GatewayError::new(
                    GatewayErrorKind::Cancelled,
                    "Parent request was cancelled.",
                ));
            }
            if authorization.deadline_at.is_elapsed() {
                return Err(GatewayError::new(
                    GatewayErrorKind::Timeout,
                    "Request deadline elapsed.",
                ));
            }
            if request_policy.is_some() {
                start_guard = Some(self.prepare_execution_start(&request, &request_id, &mut authorization).await?);
            }
            let routing_context = RoutingContext::default();
            let account_scope = Arc::clone(&authorization.account_scope);
            let plan = match &request.target {
                ExecutionTarget::ProviderEndpoint {
                    provider,
                    upstream_model,
                } => request.client.snapshot.plan_provider_endpoint(
                    provider,
                    upstream_model.as_ref(),
                    &request.operation,
                    account_scope,
                    &routing_context,
                ),
                ExecutionTarget::Model(public_model) => {
                    let mut target_model = public_model.clone();
                    let mut target_context = routing_context.clone();
                    if let Some(policy) = &request_policy {
                        let available = request
                            .client
                            .snapshot
                            .available_providers(&account_scope, &routing_context);
                        match policy
                            .route_model(request.operation.clone(), public_model.clone(), available)
                            .await
                            .map_err(|_| {
                                GatewayError::new(
                                    GatewayErrorKind::Internal,
                                    "Model routing policy failed.",
                                )
                            })? {
                            ModelRouteDecision::Unhandled => {}
                            ModelRouteDecision::Reject => {
                                return Err(GatewayError::new(
                                    GatewayErrorKind::PolicyDenied,
                                    "Model routing policy rejected the request.",
                                ));
                            }
                            ModelRouteDecision::Route { provider, model } => {
                                if provider.is_none() && model.is_none() {
                                    return Err(GatewayError::new(
                                        GatewayErrorKind::Internal,
                                        "Model routing policy returned an empty target.",
                                    ));
                                }
                                if let Some(provider) = provider {
                                    if !authorization
                                        .account_scope
                                            .provider_kinds()
                                            .contains(&provider)
                                    {
                                        return Err(GatewayError::new(
                                            GatewayErrorKind::PolicyDenied,
                                            "Model routing policy exceeded the frozen provider scope.",
                                        ));
                                    }
                                    target_context.required_provider = Some(provider);
                                }
                                if let Some(model) = model {
                                    target_model = model;
                                }
                            }
                        }
                    }
                    request.client.snapshot.plan(
                        &target_model,
                        &request.operation,
                        account_scope,
                        &target_context,
                    )
                }
            }
            .map_err(map_routing_error)?;
            let continuation = match request.metadata.previous_response_id.as_ref() {
                Some(previous) => {
                    let caller = request.client.policy.caller_identity();
                    if caller.is_unidentified() { return Err(GatewayError::new(GatewayErrorKind::PolicyDenied, "Unidentified requests cannot continue a response.")); }
                    let resolve = self
                        .continuation
                        .resolve_for_caller(request.client.policy.key_id(), &caller, previous)
                        .fuse();
                    let timeout = Delay::new(COORDINATION_TIMEOUT).fuse();
                    pin_mut!(resolve, timeout);
                    let pin = select_biased! {
                        result = resolve => match result {
                            Ok(pin) => pin,
                            Err(error)
                                if error.kind() == NativeContinuationStoreErrorKind::Unavailable =>
                            {
                                tracing::debug!(request_id = request_id.as_str(), %error, "Continuation affinity 查询失败，退化为外部续接");
                                None
                            }
                            Err(error)
                                if error.kind() == NativeContinuationStoreErrorKind::OwnershipMismatch =>
                            {
                                return Err(GatewayError::new(GatewayErrorKind::PolicyDenied, "Continuation does not belong to this client API key."));
                            }
                            Err(error) => {
                                tracing::warn!(request_id = request_id.as_str(), %error, "Continuation affinity 记录无效，已拒绝续接");
                                return Err(GatewayError::new(GatewayErrorKind::Internal, "Continuation state is invalid."));
                            }
                        },
                        _ = timeout => None,
                    };
                    Some(match pin {
                        Some(pin) if !pin.matches_caller(&caller) => {
                            return Err(GatewayError::new(GatewayErrorKind::PolicyDenied, "Continuation does not belong to this client API key."));
                        }
                        Some(pin) if !authorization.account_scope.allows(pin.account()) => {
                            return Err(GatewayError::new(GatewayErrorKind::PolicyDenied, "Continuation account is outside the client account scope."));
                        }
                        Some(pin)
                            if !plan.candidates().iter().any(|candidate| candidate.provider() == pin.provider()) =>
                        {
                            return Err(GatewayError::new(GatewayErrorKind::NoAvailableProvider, "Continuation provider is not available."));
                        }
                        Some(pin) => {
                            attach_continuation_session_state(&mut request.operation, &pin);
                            ContinuationBinding::Pinned(pin)
                        }
                        None if !matches!(caller, crate::policy::CallerIdentity::Direct { .. }) => {
                            return Err(GatewayError::new(GatewayErrorKind::PolicyDenied, "Trusted caller continuation ownership is unavailable; send complete history to start a new chain."));
                        }
                        None => ContinuationBinding::External(previous.clone()),
                    })
                }
                None => None,
            };
            if start_guard.is_none() {
                start_guard = Some(
                    self.prepare_execution_start(&request, &request_id, &mut authorization)
                        .await?,
                );
            }
            let execution_effects = authorization.execution_effects.clone();
            let extensions =
                CoordinationExtensions::new(continuation, request_observation.clone())
                    .with_caller_identity(request.client.policy.caller_identity())
                    .with_caller_token_name(request.client.policy.caller_token_name().map(str::to_owned))
                    .with_response_control(authorization.response_control.clone())
                    .with_request_policy(request_policy)
                    .with_upstream_adapters(upstream_adapters)
                    .with_execution_effects(
                        execution_effects,
                        authorization.execution_effects_baseline,
                    )
                    .with_middleware(
                        middleware,
                        Arc::from(account_group_ids.clone()),
                        request.metadata.endpoint.clone(),
                        request.metadata.transport,
                    )
                    .with_extension_scope(authorization.extension_scope.clone());
            entered_execution = true;
            self.start_without_continuation(
                request,
                PreparedExecutionStart {
                    request_id: request_id.clone(),
                    started_at,
                    plan,
                    extensions,
                    authorization,
                    guard: start_guard
                        .take()
                        .expect("execution start guard was prepared"),
                },
            )
            .await
        }
        .await;
        if !entered_execution && let Err(error) = &result {
            tracing::warn!(
                request_id = request_id.as_str(),
                key_id = budget_key_id.as_str(),
                failure_kind = error.kind().as_str(),
                "请求在执行前被拒绝"
            );
            let rejection = crate::engine::EntryRejection {
                request_id: request_id.clone(),
                client_key_id: budget_key_id.clone(),
                error: error.clone(),
                latency: started_at.elapsed().unwrap_or_default(),
            };
            let write = self.observations.record_entry_rejection(rejection).fuse();
            let timeout = Delay::new(COORDINATION_TIMEOUT).fuse();
            pin_mut!(write, timeout);
            select_biased! {
                result = write => {
                    if let Err(error) = result {
                        tracing::warn!(request_id = request_id.as_str(), operation = "record_entry_rejection", error_kind = ?error.kind(), "入口拒绝观测写入失败");
                    }
                },
                _ = timeout => tracing::warn!(request_id = request_id.as_str(), operation = "record_entry_rejection", "入口拒绝观测写入超时"),
            }
        }
        if let (Some(observation), Err(error)) = (&request_observation, &result) {
            observation.reject(error);
        }
        if result.is_err()
            && let Some(guard) = start_guard.take()
        {
            guard
                .release_failed(
                    self.budget.as_deref(),
                    request_id,
                    budget_key_id,
                    self.diagnostics.as_ref(),
                )
                .await;
        }
        result
    }

    async fn prepare_execution_start(
        &self,
        request: &PendingStartExecution,
        request_id: &ModelRequestId,
        authorization: &mut AuthorizedExecution,
    ) -> Result<ExecutionStartGuard, GatewayError> {
        let concurrency_wait_budget = ConcurrencyWaitBudget::default();
        let admission_started_at = Instant::now();
        let admission = self
            .acquire_client_admission(
                &request.client,
                request_id,
                authorization.deadline_at,
                authorization.cancellation.clone(),
                &concurrency_wait_budget,
            )
            .await?;
        let admission_decision_ms = Some(duration_ms(admission_started_at.elapsed()));
        if let Some(budget) = &self.budget
            && let Err(error) = budget.admit(request.client.policy.key_id().clone()).await
        {
            admission.release().await;
            return Err(error);
        }
        Ok(ExecutionStartGuard {
            admission,
            concurrency_wait_budget,
            admission_decision_ms,
        })
    }

    async fn start_without_continuation(
        &self,
        request: PendingStartExecution,
        prepared: PreparedExecutionStart,
    ) -> Result<StartedExecution, GatewayError> {
        let PreparedExecutionStart {
            request_id,
            started_at,
            plan,
            extensions,
            authorization,
            guard: start_guard,
        } = prepared;
        let PendingStartExecution {
            client,
            target,
            operation,
            metadata,
        } = request;
        let providers = self.providers.clone();
        let ExecutionStartGuard {
            admission,
            concurrency_wait_budget,
            admission_decision_ms,
        } = start_guard;
        let observation = plan
            .candidates()
            .first()
            .map_or_else(Default::default, |candidate| {
                providers.request_observation(
                    candidate.provider(),
                    &operation,
                    client.policy.key_id(),
                    &client.policy.caller_identity(),
                )
            });
        let new_request = NewModelRequest {
            caller_identity: Some(client.policy.caller_identity()),
            caller_token_name_snapshot: client.policy.caller_token_name().map(str::to_owned),
            id: request_id.clone(),
            client_api_key_name_snapshot: client.policy.name_snapshot().map(str::to_owned),
            client_api_key_ref: client.policy.key_id().clone(),
            config_revision: plan.config_revision(),
            routing: authorization.account_scope.routing_snapshot(),
            protocol: metadata.protocol,
            operation: operation.kind(),
            endpoint: metadata.endpoint,
            client_transport: metadata.transport.as_str().to_owned(),
            requested_model: target.into_public_model().or(observation.requested_model),
            client_ip: metadata.client_ip,
            user_agent: metadata.user_agent,
            reasoning_effort: observation.reasoning_effort,
            reasoning_preset: observation.reasoning_preset,
            request_kind: observation.request_kind,
            subagent_kind: observation.subagent_kind,
            compact: observation.compact,
            continuation: observation.continuation,
            image_generation_requested: operation.image_generation_requested(),
            admission_decision_ms,
            started_at,
            deadline_at: authorization.deadline_at,
        };
        let mut engine = GatewayEngine::new(self.observations.clone(), providers);
        if let Some(store) = &self.account_error_protection {
            engine = engine.with_account_error_protection(Arc::clone(store));
        }
        if let Some(store) = &self.client_retry {
            engine = engine.with_client_retry_store(Arc::clone(store));
        }
        let coordinator = AttemptCoordinator::new(engine);
        let core = match coordinator
            .start_observed(
                new_request,
                operation,
                plan,
                None,
                extensions,
                authorization.cancellation,
            )
            .await
        {
            Ok(core) => core.with_concurrency_wait_budget(concurrency_wait_budget),
            Err(error) => {
                if let Some(budget) = &self.budget {
                    settle_budget(
                        budget.as_ref(),
                        ClientBudgetCharge {
                            key_id: client.policy.key_id().clone(),
                            request_id: request_id.clone(),
                            amount_usd: crate::metering::Decimal::ZERO,
                            completed_at: SystemTime::now(),
                        },
                        self.diagnostics.as_ref(),
                    )
                    .await;
                }
                admission.release().await;
                return Err(gateway_error_from_engine(&error));
            }
        };
        Ok(StartedExecution {
            request_id,
            created_at: started_at,
            stream: metadata.stream,
            session: Box::new(DefaultExecutionSession::new(
                core,
                admission,
                Arc::clone(&self.continuation),
                self.budget.clone(),
                self.diagnostics.clone(),
            )),
        })
    }

    async fn acquire_client_admission(
        &self,
        client: &AuthenticatedClient,
        request_id: &ModelRequestId,
        deadline_at: Deadline,
        cancellation: CancellationToken,
        budget: &ConcurrencyWaitBudget,
    ) -> Result<AdmissionLease, GatewayError> {
        let policy = client.snapshot.client_queue_policy();
        let limits = client.policy.limits();
        let key = client.policy.key_id();
        let mut waiting =
            CapacityWait::new(&self.admission_waiting, policy, deadline_at.at(), budget);
        let mut admission = AdmissionLease {
            port: Arc::clone(&self.admissions),
            diagnostics: self.diagnostics.clone(),
            client_api_key_id: key.clone(),
            model_request_id: request_id.clone(),
            armed: false,
            renewal: None,
        };
        loop {
            let remaining = deadline_at.bounded(REQUEST_LEASE_TTL);
            if remaining.is_zero() {
                return Err(GatewayError::new(
                    GatewayErrorKind::Timeout,
                    "Request deadline elapsed.",
                ));
            }
            // 在发送原子准入之前接管取消清理，覆盖 Redis 已取得租约但返回尚未被观察的窗口
            admission.armed = true;
            let acquire = self
                .admissions
                .admit(ClientAdmissionRequest {
                    model_request_id: request_id.clone(),
                    client_api_key_id: key.clone(),
                    lease_ttl: remaining,
                    allow_concurrency_acquire: limits.max_concurrency == 0 || waiting.can_try(key),
                    limits,
                })
                .fuse();
            let timeout = deadline_at.wait().fuse();
            let cancelled = cancellation.cancelled().fuse();
            pin_mut!(acquire, timeout, cancelled);
            let decision = select_biased! {
                () = cancelled => return Err(GatewayError::new(GatewayErrorKind::Cancelled, "Request admission was cancelled.")),
                result = acquire => result.map_err(|source| GatewayError::new(GatewayErrorKind::NoAvailableProvider, "Request admission is temporarily unavailable.").with_source(source))?,
                _ = timeout => return Err(GatewayError::new(GatewayErrorKind::Timeout, "Request deadline elapsed.")),
            };
            match decision {
                ClientAdmissionDecision::Granted => {
                    admission.renewal = Some(self.admissions.maintain(
                        key,
                        request_id,
                        deadline_at,
                        cancellation.clone(),
                    ));
                    if !waiting.elapsed().is_zero() {
                        tracing::info!(
                            request_id = request_id.as_str(),
                            queue_layer = "client_key",
                            queue_wait_ms = duration_ms(waiting.elapsed()),
                            "排队请求已取得 Key 并发槽位"
                        );
                    }
                    return Ok(admission);
                }
                ClientAdmissionDecision::Rejected(reason) => {
                    admission.armed = false;
                    if reason == ClientAdmissionRejection::RateLimited || policy.max_waiting == 0 {
                        return Err(GatewayError::new(
                            GatewayErrorKind::RateLimited,
                            "Request exceeds client API key limits.",
                        ));
                    }
                    if waiting.elapsed().is_zero()
                        && let Some(budget) = &self.budget
                    {
                        budget.admit(key.clone()).await?;
                    }
                    waiting.wait(std::slice::from_ref(key)).await.map_err(|error| {
                        tracing::info!(request_id = request_id.as_str(), queue_layer = "client_key", queue_wait_ms = duration_ms(waiting.elapsed()), reason = %error, "Key 排队请求被拒绝");
                        error.gateway_error()
                    })?;
                }
            }
        }
    }

    async fn probe_inner(
        &self,
        request: AccountProbeRequest,
        frozen: Option<Arc<crate::routing::RuntimeSnapshot>>,
    ) -> Result<AccountProbeResult, AccountProbeError> {
        let AccountProbeRequest {
            target,
            provider_kind,
            upstream_model,
            operation,
        } = request;
        let observed = ProbeObservation {
            provider_kind: provider_kind.clone(),
            account_id: target.account_id().clone(),
            upstream_model: upstream_model.clone(),
        };
        let snapshot = frozen
            .map_or_else(|| self.snapshots.acquire(), Ok)
            .map_err(|_| {
                GatewayError::new(
                    GatewayErrorKind::Internal,
                    "Gateway configuration is unavailable.",
                )
            })?;
        let public_model =
            PublicModelId::new(upstream_model.as_str().to_owned()).map_err(|_| {
                GatewayError::new(GatewayErrorKind::Unsupported, "Requested model is invalid.")
            })?;
        let routing_context = RoutingContext {
            required_provider: Some(provider_kind),
            ..RoutingContext::default()
        };
        let plan = snapshot
            .plan_diagnostic(&public_model, &operation, &routing_context)
            .map_err(map_routing_error)?;
        let started_at = SystemTime::now();
        let deadline_at = started_at.checked_add(DIAGNOSTIC_TIMEOUT).ok_or_else(|| {
            GatewayError::new(GatewayErrorKind::Internal, "System clock is invalid.")
        })?;
        let request_id = new_request_id()?;
        let actor = ClientApiKeyId::new("admin_connection_test")
            .map_err(|_| GatewayError::new(GatewayErrorKind::Internal, "Invalid admin actor."))?;
        let new_request = NewModelRequest {
            caller_identity: None,
            caller_token_name_snapshot: None,
            id: request_id,
            client_api_key_name_snapshot: None,
            client_api_key_ref: actor,
            config_revision: plan.config_revision(),
            routing: crate::routing::AccountRoutingSnapshot::all(),
            protocol: "admin_connection_test".to_owned(),
            operation: operation.kind(),
            endpoint: "/api/admin/accounts/connection-test".to_owned(),
            client_transport: ClientTransport::InternalProbe.as_str().to_owned(),
            requested_model: Some(public_model),
            client_ip: None,
            user_agent: None,
            reasoning_effort: None,
            reasoning_preset: None,
            request_kind: Some("account_connection_test".to_owned()),
            subagent_kind: None,
            compact: false,
            continuation: Default::default(),
            image_generation_requested: false,
            admission_decision_ms: None,
            started_at,
            deadline_at: deadline_at.into(),
        };
        let providers = self.providers.clone();
        let transient: Arc<dyn ExecutionStore> = Arc::new(TransientExecutionStore);
        let coordinator = AttemptCoordinator::new(GatewayEngine::new(transient, providers));
        let mut session = match coordinator
            .start_diagnostic(
                new_request,
                operation,
                plan,
                target,
                None,
                CancellationToken::new(),
            )
            .await
        {
            Ok(session) => session,
            Err(error) => {
                return Err(self
                    .observe_probe_failure(&observed, started_at, &error)
                    .await);
            }
        };
        let events = session.collect_uncommitted().await;
        let events = match events {
            Ok(events) => events,
            Err(error) => {
                return Err(self
                    .observe_probe_failure(&observed, started_at, &error)
                    .await);
            }
        };
        if let Err(error) = session.commit_downstream(Some(200)).await {
            return Err(self
                .observe_probe_failure(&observed, started_at, &error)
                .await);
        }
        Ok(AccountProbeResult {
            text: events
                .into_iter()
                .flat_map(|event| event.into_parts().0)
                .filter_map(|fact| match fact {
                    GatewayEvent::TextDelta(delta) => Some(delta.text),
                    _ => None,
                })
                .collect(),
        })
    }

    /// 探测失败先记录脱敏分类事实，再把请求局部的原始上游响应交给认证管理端
    async fn observe_probe_failure(
        &self,
        observed: &ProbeObservation,
        started_at: SystemTime,
        error: &EngineError,
    ) -> AccountProbeError {
        let (source, send_state, upstream_response) = match error {
            EngineError::Provider(provider_error) => {
                let upstream_response = provider_error
                    .client_visible_upstream_response()
                    .map(AccountProbeUpstreamResponse::from_client_response);
                let has_upstream_facts = provider_error.send_state() != UpstreamSendState::NotSent
                    || provider_error.upstream_status().is_some()
                    || provider_error.upstream_code().is_some()
                    || provider_error.client_visible_upstream_error().is_some()
                    || upstream_response.is_some();
                let source = if has_upstream_facts {
                    AccountProbeErrorSource::Upstream
                } else {
                    AccountProbeErrorSource::Provider
                };
                (source, Some(provider_error.send_state()), upstream_response)
            }
            _ => (AccountProbeErrorSource::Gateway, None, None),
        };
        if let EngineError::Provider(provider_error) = error {
            let latency = started_at.elapsed().unwrap_or_default();
            let latency_ms = u64::try_from(latency.as_millis()).unwrap_or(u64::MAX);
            tracing::warn!(
                target: "gateway_probe",
                provider_kind = observed.provider_kind.as_str(),
                account_id = observed.account_id.as_str(),
                upstream_model = observed.upstream_model.as_str(),
                failure_kind = provider_error.kind().as_str(),
                send_state = ?provider_error.send_state(),
                upstream_status = ?provider_error.upstream_status(),
                diagnostic_stage = ?provider_error.diagnostic().and_then(|diagnostic| diagnostic.stage()),
                diagnostic_code = ?provider_error.diagnostic().and_then(|diagnostic| diagnostic.code()),
                latency_ms,
                "账号连接测试失败"
            );
            if let Err(store_error) = self
                .observations
                .record_probe_failure(ProbeFailure {
                    provider_kind: observed.provider_kind.clone(),
                    account_id: observed.account_id.clone(),
                    upstream_model_id: observed.upstream_model.clone(),
                    error: provider_error.stable_snapshot(),
                    latency,
                })
                .await
            {
                tracing::warn!(
                    operation = "record_probe_failure",
                    provider_kind = observed.provider_kind.as_str(),
                    account_id = observed.account_id.as_str(),
                    error_kind = ?store_error.kind(),
                    "账号连接测试观测写入失败，测试结果不受影响"
                );
            }
        }
        AccountProbeError::new(
            gateway_error_from_engine(error),
            source,
            send_state,
            upstream_response,
        )
    }
}

impl ExecutionService for DefaultExecutionService {
    fn request_settings(&self) -> Option<RequestSettings> {
        self.snapshots.acquire().ok().map(RequestSettings::new)
    }

    fn authenticate(
        &self,
        plaintext: &str,
    ) -> Result<AuthenticatedClient, ClientAuthenticationError> {
        let client = self.authenticate_without_usage(plaintext)?;
        self.client_api_key_usage
            .record_used(client.policy().key_id());
        Ok(client)
    }

    fn authenticate_request(
        &self,
        request: ClientAuthenticationRequest,
    ) -> BoxFuture<'_, Result<AuthenticatedClient, ClientAuthenticationError>> {
        Box::pin(async move {
            let client = self.authenticate_request_without_usage(request).await?;
            self.client_api_key_usage
                .record_used(client.policy().key_id());
            Ok(client)
        })
    }

    fn verify_request(
        &self,
        request: ClientAuthenticationRequest,
    ) -> BoxFuture<'_, Result<AuthenticatedClient, ClientAuthenticationError>> {
        Box::pin(self.authenticate_request_without_usage(request))
    }

    fn public_models(&self, client: &AuthenticatedClient) -> Vec<PublicModelId> {
        client
            .snapshot
            .public_models_for_scope(client.policy.account_scope())
    }

    fn client_model_catalog<'a>(
        &'a self,
        client: &'a AuthenticatedClient,
        protocol: &'a str,
        client_version: &'a str,
    ) -> BoxFuture<'a, Result<Vec<PublicModelDescriptor>, ProviderCatalogUnavailable>> {
        Box::pin(async move {
            let scope = client.policy.account_scope();
            let providers = &self.providers;
            let mut result = Vec::new();
            let mut seen = BTreeSet::new();
            for kind in scope.provider_kinds() {
                let provider = providers.get(kind).ok_or(ProviderCatalogUnavailable)?;
                let Some(models) = provider
                    .query_client_model_catalog(
                        client.policy.key_id(),
                        scope,
                        protocol,
                        client_version,
                    )
                    .await?
                else {
                    for profile in client.snapshot.profiles_for_provider_scope(kind, scope) {
                        if seen.insert(profile.model().clone()) {
                            result.push(PublicModelDescriptor::Adapted(profile));
                        }
                    }
                    continue;
                };
                if models.iter().any(|entry| entry.source_account.is_some()) {
                    // 完整原生对象优先，稳定排序保留各来源的上游顺序，额外别名随后补入
                    let mut sources = models.iter().collect::<Vec<_>>();
                    sources.sort_by_key(|entry| {
                        matches!(entry.content, ProviderModelContent::Adapted(_))
                    });
                    let ordered_models = sources
                        .iter()
                        .map(|entry| {
                            PublicModelId::new(entry.model.as_str())
                                .map_err(|_| ProviderCatalogUnavailable)
                        })
                        .chain(
                            client
                                .snapshot
                                .public_models_for_provider(kind)
                                .into_iter()
                                .map(Ok),
                        );
                    for model in ordered_models {
                        let model = model?;
                        if seen.contains(&model) {
                            continue;
                        }
                        let source = sources.iter().find(|entry| {
                            entry.source_account.as_ref().is_some_and(|account| {
                                scope.account_provider(account) == Some(kind)
                                    && client
                                        .snapshot
                                        .allows_public_model_for_account(&model, scope, account)
                                    && client
                                        .snapshot
                                        .upstream_model_for_account(&model, scope, account)
                                        == entry.model.as_str()
                            })
                        });
                        if let Some(source) = source {
                            seen.insert(model.clone());
                            result.push(source.content.for_public_model(model));
                        }
                    }
                    for profile in client.snapshot.contributed_profiles_for_scope(kind, scope) {
                        if seen.insert(profile.model().clone()) {
                            result.push(PublicModelDescriptor::Adapted(profile));
                        }
                    }
                    continue;
                }
                // 保留上游顺序；映射只选择完整的目标对象，不能跨账号/模型混拼字段
                let by_id = models
                    .iter()
                    .map(|entry| (entry.model.as_str(), entry))
                    .collect::<std::collections::BTreeMap<_, _>>();
                for entry in &models {
                    let target = client.snapshot.mapped_model(entry.model.as_str());
                    if !scope.allows_provider_model(kind, &target) {
                        continue;
                    }
                    let Some(source) = by_id.get(target.as_str()) else {
                        continue;
                    };
                    let model = PublicModelId::new(entry.model.as_str().to_owned())
                        .map_err(|_| ProviderCatalogUnavailable)?;
                    // 原生目录可能比路由快照更新，不能向客户端公布当前已知不可路由的模型
                    if !client
                        .snapshot
                        .contains_public_model_for_provider(&model, kind)
                    {
                        continue;
                    }
                    if seen.insert(model.clone()) {
                        result.push(source.content.for_public_model(model));
                    }
                }
                for model in client.snapshot.public_models_for_provider(kind) {
                    let target = client.snapshot.mapped_model(model.as_str());
                    if !scope.allows_provider_model(kind, &target) {
                        continue;
                    }
                    if target == model.as_str() || seen.contains(&model) {
                        continue;
                    }
                    if !client
                        .snapshot
                        .contains_public_model_for_provider(&model, kind)
                    {
                        continue;
                    }
                    if let Some(source) = by_id.get(target.as_str()) {
                        seen.insert(model.clone());
                        result.push(source.content.for_public_model(model));
                    }
                }
                // 原生完整对象优先；只补入当前授权账号实际提供的独立适配画像
                for profile in client.snapshot.contributed_profiles_for_scope(kind, scope) {
                    if seen.insert(profile.model().clone()) {
                        result.push(PublicModelDescriptor::Adapted(profile));
                    }
                }
            }
            Ok(result)
        })
    }

    fn contains_public_model(&self, client: &AuthenticatedClient, model: &PublicModelId) -> bool {
        client
            .snapshot
            .contains_public_model_for_scope(model, client.policy.account_scope())
    }

    fn prepare_execution(
        &self,
        client: AuthenticatedClient,
    ) -> BoxFuture<'_, Result<PreparedRootExecution, GatewayError>> {
        Box::pin(async move { self.prepare_root_execution_inner(client).await })
    }

    fn middleware_plan(&self, prepared: &PreparedRootExecution) -> Option<FrozenMiddlewarePlan> {
        let generation = prepared.client.snapshot.extensions()?.clone();
        self.execution_extensions.as_ref()?.middleware(&generation)
    }

    fn start_prepared(
        &self,
        prepared: PreparedRootExecution,
        request: PreparedExecutionRequest,
    ) -> BoxFuture<'_, Result<StartedExecution, GatewayError>> {
        Box::pin(async move {
            self.start_prepared_with_target(
                prepared,
                ExecutionTarget::Model(request.public_model),
                request.operation,
                request.metadata,
            )
            .await
        })
    }

    fn start_prepared_provider_endpoint(
        &self,
        prepared: PreparedRootExecution,
        provider: ProviderKind,
        upstream_model: Option<UpstreamModelId>,
        operation: Operation,
        metadata: ExecutionRequestMetadata,
    ) -> BoxFuture<'_, Result<StartedExecution, GatewayError>> {
        Box::pin(async move {
            self.start_prepared_with_target(
                prepared,
                ExecutionTarget::ProviderEndpoint {
                    provider,
                    upstream_model,
                },
                operation,
                metadata,
            )
            .await
        })
    }

    fn start(
        &self,
        request: StartExecution,
    ) -> BoxFuture<'_, Result<StartedExecution, GatewayError>> {
        Box::pin(async move { self.start_inner(request).await })
    }

    fn start_provider_endpoint(
        &self,
        request: StartProviderExecution,
    ) -> BoxFuture<'_, Result<StartedExecution, GatewayError>> {
        Box::pin(async move { self.start_provider_endpoint_inner(request).await })
    }

    fn live_gateway(&self) -> Option<Arc<dyn crate::live::LiveGateway>> {
        self.providers
            .iter()
            .find_map(|provider| provider.live_gateway())
    }
}

impl ClientKeyVerifier for DefaultExecutionService {
    fn verify_client_key(
        &self,
        plaintext: &str,
    ) -> Result<ClientApiKeyId, ClientAuthenticationError> {
        self.authenticate_without_usage(plaintext)
            .map(|client| client.policy().key_id().clone())
    }
}

impl AccountProbe for DefaultExecutionService {
    fn probe(
        &self,
        request: AccountProbeRequest,
        snapshot: Option<Arc<crate::routing::RuntimeSnapshot>>,
    ) -> BoxFuture<'_, Result<AccountProbeResult, AccountProbeError>> {
        Box::pin(async move { self.probe_inner(request, snapshot).await })
    }
}

fn constant_time_equal(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.as_bytes()
        .iter()
        .zip(right.as_bytes())
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn native_policy(
    snapshot: &RuntimeSnapshot,
    authentication: &ClientAuthenticationRequest,
) -> Result<ClientPolicy, ClientAuthenticationError> {
    let plaintext = authentication
        .native_bearer()
        .ok_or(ClientAuthenticationError::InvalidKey)?
        .expose_for_auth();
    snapshot
        .client_policies()
        .filter(|policy| constant_time_equal(plaintext, policy.plaintext_key().expose_for_auth()))
        .find(|policy| policy.authorize().is_ok())
        .cloned()
        .ok_or(ClientAuthenticationError::InvalidKey)
}

fn attach_continuation_session_state(operation: &mut Operation, pin: &NativeContinuationPin) {
    let Some(state) = pin.session_state() else {
        return;
    };
    if state.provider() != pin.provider().as_str()
        || operation.provider_session_state(state.provider()).is_some()
    {
        return;
    }
    operation.set_provider_session_state(state.clone());
}
