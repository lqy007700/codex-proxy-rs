//! 智谱账号范围、通用调度租约与冷启动 Responses 请求

use crate::{
    credential::Credential,
    response::{Decoder, error, upstream_error},
};
use async_trait::async_trait;
use futures::StreamExt;
use gateway_core::{
    account::{
        AccountCandidate, AccountCapacitySnapshot, AccountEligibilityPolicy,
        AccountHistorySnapshot, AccountSelectionContext, AccountSelector, LoadedCredential,
        ProviderAccountId,
    },
    concurrency::{CapacityWait, ConcurrencyWaitQueue},
    engine::{
        AttemptContext, ContinuationAttempt,
        provider::{
            Provider, ProviderCallMetadata, ProviderRequest, ProviderSelectionObservation,
            ProviderStream,
        },
    },
    error::{
        ContinuationFailure, ContinuationRecoveryDisposition, ProviderError, ProviderErrorKind,
    },
    event::{ProviderEvent, ProviderResponseObservation, ProviderResponseTimings},
    operation::{Feature, GenerateRequest, Operation, OperationKind, ProviderSessionState},
    provider_ports::{
        ProviderConcurrencyPool, ProviderCooldown, ProviderLeaseAcquisition, ProviderLeaseGuard,
        ProviderLeaseRequest, ProviderSchedulingLeaseRequest,
    },
    routing::{
        ModelCapabilities, ModelPresentation, ProviderCatalogGeneration, ProviderKind,
        ProviderModelCapabilities, SupportLevel, UpstreamModelId,
    },
    upstream::{UpstreamSendState, UpstreamTransport},
};
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

/// 通过标准 API Key 执行智谱 Responses，不持有其它 Provider 的状态
pub struct GlmProvider {
    kind: ProviderKind,
    accounts: Arc<dyn gateway_core::account::ProviderAccountStore>,
    leases: Arc<dyn gateway_core::provider_ports::ProviderLeasePort>,
    cooldowns: Arc<dyn gateway_core::provider_ports::ProviderCooldownPort>,
    feedback: Arc<gateway_core::account::AccountFeedbackStats>,
    waiting: ConcurrencyWaitQueue<ProviderAccountId>,
}
struct Selected {
    loaded: LoadedCredential,
    guard: Box<dyn ProviderLeaseGuard>,
    capacity: Option<AccountCapacitySnapshot>,
    wait_ms: u64,
}
impl GlmProvider {
    /// 只接收智谱使用的账号、租约和冷却端口
    pub fn new(
        kind: ProviderKind,
        accounts: Arc<dyn gateway_core::account::ProviderAccountStore>,
        leases: Arc<dyn gateway_core::provider_ports::ProviderLeasePort>,
        cooldowns: Arc<dyn gateway_core::provider_ports::ProviderCooldownPort>,
        feedback: Arc<gateway_core::account::AccountFeedbackStats>,
    ) -> Self {
        Self {
            kind,
            accounts,
            leases,
            cooldowns,
            feedback,
            waiting: ConcurrencyWaitQueue::default(),
        }
    }

    async fn select(
        &self,
        model: &UpstreamModelId,
        context: &AttemptContext,
    ) -> Result<Selected, ProviderError> {
        let start = Instant::now();
        let leases = self.leases.clone();
        let accounts = self.accounts.clone();
        let cooldowns = self.cooldowns.clone();
        let mut waiting = CapacityWait::new(
            &self.waiting,
            context.account_selection_policy().queue_policy(),
            context.deadline().at(),
            context.concurrency_wait_budget(),
        );
        loop {
            preflight(context)?;
            let mut available = accounts
                .list_for_provider(&self.kind)
                .await
                .map_err(|_| infrastructure())?;
            if context.is_diagnostic_required_account()
                && let Some(id) = context.required_account()
                && !available.iter().any(|a| a.id() == id)
                && let Some(account) = accounts
                    .get_account(id)
                    .await
                    .map_err(|_| infrastructure())?
                && account.provider() == &self.kind
            {
                available.push(account);
            }
            available.retain(|account| {
                account.provider() == &self.kind
                    && account.authentication_kind() == "api_key"
                    && context
                        .required_account()
                        .is_none_or(|id| id == account.id())
                    && (context.is_diagnostic_required_account()
                        || context
                            .account_scope()
                            .is_some_and(|scope| scope.allows_model(account.id(), model.as_str())))
            });
            let mut model_accounts = Vec::with_capacity(available.len());
            for account in available {
                let loaded = accounts
                    .load_credential(account.id(), account.revision())
                    .await
                    .map_err(|_| infrastructure())?;
                let credential = Credential::parse(loaded.credential.expose_to_provider())
                    .map_err(|_| infrastructure())?;
                if credential.models.iter().any(|id| id == model.as_str()) {
                    model_accounts.push(account);
                }
            }
            let available = model_accounts;
            let ids = available.iter().map(|a| a.id().clone()).collect::<Vec<_>>();
            let state = leases
                .load_state(
                    context.client_api_key_ref(),
                    &self.kind,
                    &ids,
                    ProviderConcurrencyPool::Shared,
                )
                .await
                .map_err(|_| infrastructure())?;
            let mut candidates = Vec::with_capacity(available.len());
            for account in available {
                let cooldown = if context.is_diagnostic_required_account() {
                    None
                } else {
                    cooldowns
                        .read(account.id())
                        .await
                        .map_err(|_| infrastructure())?
                        .filter(|c| c.credential_revision() == account.revision())
                        .map(|c| c.scheduling_state())
                };
                let health = self.feedback.scheduling_signals(&self.kind, account.id());
                let signals = state
                    .signals()
                    .get(account.id())
                    .cloned()
                    .ok_or_else(infrastructure)?
                    .with_rate_limit(cooldown)
                    .with_runtime_health(health.0, health.1);
                candidates.push(AccountCandidate { account, signals });
            }
            let diagnostic = context.is_diagnostic_required_account();
            let mut selection = AccountSelectionContext {
                policy: context.account_selection_policy(),
                now: SystemTime::now(),
                excluded_accounts: context.excluded_accounts().clone(),
                busy_accounts: BTreeSet::new(),
                preferred_account: context.required_account().cloned(),
                preferred_account_overrides_weight: false,
                round_robin_cursor: state.round_robin_cursor(),
                eligibility: if diagnostic {
                    AccountEligibilityPolicy::BypassForDiagnostic
                } else {
                    AccountEligibilityPolicy::Enforce
                },
                account_scope: context.account_scope().cloned(),
                reserved_concurrency: 0,
            };
            if let Some(pin) = context.continuation().and_then(|c| c.pinned()) {
                if pin.provider() != &self.kind
                    || !pin.matches_client(context.client_api_key_ref())
                    || context.continuation_attempt() != ContinuationAttempt::Native
                {
                    return Err(replay_required());
                }
                candidates.retain(|candidate| candidate.account.id() == pin.account());
            }
            for candidate in &candidates {
                if !waiting.can_try(candidate.account.id()) {
                    selection
                        .busy_accounts
                        .insert(candidate.account.id().clone());
                }
            }
            let mut busy = false;
            while let Some(chosen) = context
                .select_account(
                    &self.kind,
                    Some(model.as_str()),
                    &candidates,
                    &mut selection,
                )
                .await
                .map_err(|_| {
                    error(
                        ProviderErrorKind::RequestPolicyDenied,
                        UpstreamSendState::NotSent,
                    )
                })?
            {
                let account = &chosen.candidate().account;
                let capacity = AccountSelector
                    .capacity_snapshot(&candidates, &selection)
                    .map(AccountCapacitySnapshot::with_acquired_request);
                let lease = leases
                    .try_acquire(ProviderLeaseRequest::Scheduling(
                        ProviderSchedulingLeaseRequest::new(
                            self.kind.clone(),
                            account.id().clone(),
                            account.revision(),
                            account.effective_concurrency(
                                context
                                    .account_selection_policy()
                                    .max_concurrent_per_account(),
                            ),
                            context.deadline(),
                        )
                        .with_metadata(context.lease_metadata())
                        .with_cancellation(context.cancellation().clone()),
                    ))
                    .await
                    .map_err(|_| infrastructure())?;
                match lease {
                    ProviderLeaseAcquisition::Busy { .. } => {
                        selection.busy_accounts.insert(account.id().clone());
                        busy = true;
                    }
                    ProviderLeaseAcquisition::Acquired(guard) => {
                        let loaded = accounts
                            .load_credential(account.id(), account.revision())
                            .await
                            .map_err(|_| infrastructure())?;
                        if loaded.account.provider() != &self.kind
                            || loaded.account.authentication_kind() != "api_key"
                        {
                            return Err(infrastructure());
                        }
                        return Ok(Selected {
                            loaded,
                            guard,
                            capacity,
                            wait_ms: start.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
                        });
                    }
                }
            }
            let wait_candidates = AccountSelector.wait_candidates(&candidates, &selection);
            if context
                .account_selection_policy()
                .queue_policy()
                .max_waiting
                > 0
                && !wait_candidates.is_empty()
            {
                tokio::select! {
                    biased;
                    _=context.cancellation().cancelled()=>return Err(error(ProviderErrorKind::Cancelled,UpstreamSendState::NotSent)),
                    result=AccountSelector.wait_for_capacity(&mut waiting,&wait_candidates,&candidates,&selection)=>result.map_err(|reason|error(match reason {gateway_core::concurrency::QueueRejection::Full=>ProviderErrorKind::ConcurrencyQueueFull,gateway_core::concurrency::QueueRejection::Timeout=>ProviderErrorKind::ConcurrencyQueueTimeout},UpstreamSendState::NotSent))?,
                }
                continue;
            }
            return Err(error(
                if busy || !wait_candidates.is_empty() {
                    ProviderErrorKind::AccountCapacityUnavailable
                } else {
                    ProviderErrorKind::NoEligibleAccount
                },
                UpstreamSendState::NotSent,
            ));
        }
    }

    fn execute_selected(
        self: Arc<Self>,
        operation: Operation,
        headers: Vec<gateway_core::engine::middleware::MiddlewareHeader>,
        selected: Selected,
        context: AttemptContext,
        model: UpstreamModelId,
    ) -> Result<ProviderStream, ProviderError> {
        let Operation::Generate(generate) = operation else {
            return Err(error(
                ProviderErrorKind::Unsupported,
                UpstreamSendState::NotSent,
            ));
        };
        let body = prepare_body(&generate, &model, &context)?;
        let credential = Credential::parse(selected.loaded.credential.expose_to_provider())
            .map_err(|_| infrastructure())?;
        if !credential.models.iter().any(|id| id == model.as_str()) {
            return Err(error(
                ProviderErrorKind::InvalidRequest,
                UpstreamSendState::NotSent,
            ));
        }
        if let Some(pin) = context.continuation().and_then(|c| c.pinned()) {
            let revision = pin
                .session_state()
                .and_then(|s| s.payload().get("credential_revision"))
                .and_then(Value::as_u64);
            if !pin.matches(&self.kind, selected.loaded.account.id())
                || revision != Some(selected.loaded.account.revision().get())
            {
                return Err(replay_required());
            }
        }
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .connect_timeout(Duration::from_secs(30));
        if let Some(proxy) = selected.loaded.account.outbound_proxy() {
            builder = builder
                .proxy(reqwest::Proxy::all(proxy.expose_url()).map_err(|_| infrastructure())?);
        }
        let client = builder.build().map_err(|_| infrastructure())?;
        let transport = UpstreamTransport::new("http_sse").map_err(|_| infrastructure())?;
        let metadata = ProviderCallMetadata::new(
            self.kind.clone(),
            model.clone(),
            selected.loaded.account.id().clone(),
            transport.clone(),
        )
        .with_account_snapshot(Some(AccountHistorySnapshot::from_account(
            &selected.loaded.account,
        )))
        .with_selection_observation(ProviderSelectionObservation::new(
            selected.wait_ms,
            selected.capacity,
        ));
        let revision = selected.loaded.account.revision();
        let account_id = selected.loaded.account.id().clone();
        let persisted = body.get("store").and_then(Value::as_bool) == Some(true);
        let feedback = self.feedback.clone();
        let events = async_stream::try_stream! {
            preflight(&context)?;
            let mut request = client.post(credential.responses_url()).json(&body);
            for header in headers {
                let name = header.name().to_ascii_lowercase();
                if !gateway_protocol::openai::is_transport_managed_request_header(&name)
                    && !matches!(
                        name.as_str(),
                        "authorization"
                            | "x-api-key"
                            | "cookie"
                            | "accept"
                            | "content-type"
                            | "chatgpt-account-id"
                            | "openai-organization"
                            | "openai-project"
                    )
                {
                    let name =
                        reqwest::header::HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
                            error(
                                ProviderErrorKind::InvalidRequest,
                                UpstreamSendState::NotSent,
                            )
                        })?;
                    let value =
                        reqwest::header::HeaderValue::from_bytes(header.value()).map_err(|_| {
                            error(
                                ProviderErrorKind::InvalidRequest,
                                UpstreamSendState::NotSent,
                            )
                        })?;
                    request = request.header(name, value);
                }
            }
            request = request
                .bearer_auth(&credential.api_key)
                .header("Accept", "text/event-stream")
                .header("Content-Type", "application/json");
            let mut response = tokio::select! {
                    biased;
                _=context.cancellation().cancelled()=>Err(error(ProviderErrorKind::Cancelled,UpstreamSendState::Ambiguous)),
                _=context.deadline().wait()=>Err(error(ProviderErrorKind::Timeout,UpstreamSendState::Ambiguous)),
                result=request.send()=>result.map_err(|failure|error(ProviderErrorKind::Transport,if failure.is_connect(){UpstreamSendState::NotSent}else{UpstreamSendState::Ambiguous})),
            }?;
            let status = response.status().as_u16();
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(gateway_protocol::openai::parse_retry_after_seconds)
                .map(Duration::from_secs);
            let observation = ProviderResponseObservation::new(transport).with_status_code(status);
            yield ProviderEvent::observation(observation.clone());
            if !response.status().is_success() {
                let value = read_error_body(&mut response, &context).await?;
                let mut failure = upstream_error(status, &value);
                if let Some(delay) = retry_after {
                    failure = failure.with_retry_after(delay);
                }
                if failure.kind() == ProviderErrorKind::Unauthorized && (!context.is_diagnostic_required_account() || selected.loaded.account.enabled()) {
                    let _ = self.accounts.apply_state_change(gateway_core::account::AccountStateChange {
                        account_id: account_id.clone(), expected_revision: revision,
                        credential_state: gateway_core::account::CredentialState::Invalid,
                        observed_at: SystemTime::now(), error_reason: Some(gateway_core::account::AccountErrorReason::CredentialInvalid),
                        message: Some("智谱 API Key 认证失败".to_owned()),
                    }).await;
                }
                let duration = match failure.kind() {
                    ProviderErrorKind::QuotaExhausted => Some(Duration::from_secs(30 * 60)),
                    ProviderErrorKind::Unauthorized => Some(Duration::from_secs(10 * 60)),
                    ProviderErrorKind::RateLimited => {
                        Some(retry_after.unwrap_or(Duration::from_secs(60)))
                    }
                    _ => None,
                };
                if !context.is_diagnostic_required_account()
                    && let Some(duration) = duration
                {
                    let _ = self
                        .cooldowns
                        .clone()
                        .put_if_later(ProviderCooldown::new(
                            account_id.clone(),
                            revision,
                            SystemTime::now() + duration.min(Duration::from_secs(24 * 3600)),
                        ))
                        .await;
                }
                Err(failure)?;
            }
            if !response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.starts_with("text/event-stream"))
            {
                Err(error(ProviderErrorKind::Protocol, UpstreamSendState::Sent))?;
            }
            let mut stream = response.bytes_stream();
            let mut decoder = Decoder::new(model.as_str().to_owned(), context.pricing().get("glm").and_then(|models| models.get(model.as_str())).cloned());
            let mut timings = ProviderResponseTimings::default();
            loop {
                let chunk = tokio::select! {
                    biased;
                    _=context.cancellation().cancelled()=>Err(error(ProviderErrorKind::Cancelled,UpstreamSendState::Sent)),
                    _=context.deadline().wait()=>Err(error(ProviderErrorKind::Timeout,UpstreamSendState::Sent)),
                    chunk=stream.next()=>Ok(chunk),
                }?;
                let ending = chunk.is_none();
                let decoded = match chunk {
                    Some(chunk) => decoder.push(&chunk.map_err(|_| {
                        error(ProviderErrorKind::Transport, UpstreamSendState::Sent)
                    })?),
                    None => decoder.finish(),
                };
                for event in decoded {
                    let mut event = event?;
                    let elapsed = context
                        .timing_started_at()
                        .elapsed()
                        .as_millis()
                        .try_into()
                        .unwrap_or(u64::MAX);
                    timings.first_event_ms.get_or_insert(elapsed);
                    for fact in event.canonical_facts() {
                        match fact {
                            gateway_core::event::GatewayEvent::TextDelta(_) => {
                                timings.first_text_ms.get_or_insert(elapsed);
                                timings.first_token_ms.get_or_insert(elapsed);
                            }
                            gateway_core::event::GatewayEvent::ReasoningDelta(_) => {
                                timings.first_reasoning_ms.get_or_insert(elapsed);
                                timings.first_token_ms.get_or_insert(elapsed);
                            }
                            _ => {}
                        }
                    }
                    let mut observed = observation.clone().with_timings(timings);
                    if let Some(model) = event.wire_event().and_then(|wire| wire.data().pointer("/response/model")).and_then(Value::as_str) {
                        observed = observed.with_upstream_response_model_if_valid(model);
                    }
                    event.attach_observation(observed);
                    if persisted
                        && event
                            .canonical_facts()
                            .iter()
                            .any(|f| matches!(f, gateway_core::event::GatewayEvent::Completed(_)))
                    {
                        let state = ProviderSessionState::new(
                            "glm",
                            Map::from_iter([(
                                "credential_revision".to_owned(),
                                Value::from(revision.get()),
                            )]),
                        )
                        .map_err(|_| infrastructure())?;
                        event.attach_session_update(state);
                    }
                    yield event;
                }
                if ending || decoder.completed {
                    break;
                }
            }
        };
        Ok(ProviderStream::new(metadata, events, selected.guard).with_account_feedback(feedback))
    }
}

#[async_trait]
impl Provider for GlmProvider {
    fn name(&self) -> &str {
        "glm"
    }
    fn catalog_generation(&self) -> ProviderCatalogGeneration {
        ProviderCatalogGeneration::new(0)
    }
    async fn query_model_capabilities(
        &self,
    ) -> Result<Vec<ProviderModelCapabilities>, ProviderError> {
        let mut models = BTreeMap::<String, BTreeSet<ProviderAccountId>>::new();
        for account in self
            .accounts
            .list_for_provider(&self.kind)
            .await
            .map_err(|_| infrastructure())?
        {
            if account.provider() != &self.kind || account.authentication_kind() != "api_key" {
                continue;
            }
            let loaded = self
                .accounts
                .load_credential(account.id(), account.revision())
                .await
                .map_err(|_| infrastructure())?;
            let credential = Credential::parse(loaded.credential.expose_to_provider())
                .map_err(|_| infrastructure())?;
            for model in &credential.models {
                models
                    .entry(model.clone())
                    .or_default()
                    .insert(account.id().clone());
            }
        }
        models
            .into_iter()
            .map(|(model, accounts)| {
                let capabilities =
                    ModelCapabilities::new(BTreeSet::from([OperationKind::Generate]), Some(131072))
                        .with_feature(Feature::Reasoning, SupportLevel::Native)
                        .with_feature(Feature::Tools, SupportLevel::Native)
                        .with_feature(Feature::NativeContinuation, SupportLevel::Native);
                let presentation =
                    ModelPresentation::new(Some(model.clone()), None).with_agent_tools(true, false);
                Ok(ProviderModelCapabilities::new(
                    UpstreamModelId::new(model).map_err(|_| infrastructure())?,
                    capabilities,
                )
                .with_presentation(presentation)
                .with_catalog_accounts(accounts))
            })
            .collect()
    }
    async fn execute(
        self: Arc<Self>,
        request: ProviderRequest,
        context: AttemptContext,
    ) -> Result<ProviderStream, ProviderError> {
        if request.candidate().provider() != &self.kind {
            return Err(error(
                ProviderErrorKind::InvalidRequest,
                UpstreamSendState::NotSent,
            ));
        }
        let model = request
            .candidate()
            .upstream_model()
            .cloned()
            .ok_or_else(|| {
                error(
                    ProviderErrorKind::InvalidRequest,
                    UpstreamSendState::NotSent,
                )
            })?;
        let Operation::Generate(generate) = request.operation() else {
            return Err(error(
                ProviderErrorKind::Unsupported,
                UpstreamSendState::NotSent,
            ));
        };
        prepare_body(generate, &model, &context)?;
        if context.upstream_adapter(&self.kind, &model)?.is_some() {
            return Err(error(
                ProviderErrorKind::Unsupported,
                UpstreamSendState::NotSent,
            ));
        }
        let selected = self.select(&model, &context).await?;
        let owner = self.clone();
        let frozen = context.clone();
        let kind = self.kind.clone();
        let account = selected.loaded.account.id().clone();
        context
            .execute_middleware(
                request.operation().clone(),
                kind,
                Some(model.as_str().to_owned()),
                account,
                Box::new(move |operation, headers| {
                    Box::pin(async move {
                        owner.execute_selected(operation, headers, selected, frozen, model)
                    })
                }),
            )
            .await
    }
}

pub(crate) fn prepare_body(
    generate: &GenerateRequest,
    model: &UpstreamModelId,
    context: &AttemptContext,
) -> Result<Map<String, Value>, ProviderError> {
    let payload = generate.protocol_payload();
    if payload.protocol() != "openai" {
        return Err(error(
            ProviderErrorKind::Unsupported,
            UpstreamSendState::NotSent,
        ));
    }
    if gateway_protocol::openai::codex_responses_request_semantics(
        payload.body(),
        payload.context(),
    )
    .compact
    {
        return Err(error(
            ProviderErrorKind::Unsupported,
            UpstreamSendState::NotSent,
        ));
    }
    let mut body = payload.body().clone();
    if body
        .get("previous_response_id")
        .is_some_and(|v| !v.is_null())
    {
        let pin = context
            .continuation()
            .and_then(|c| c.pinned())
            .ok_or_else(replay_required)?;
        if pin.provider().as_str() != "glm"
            || !pin.matches_client(context.client_api_key_ref())
            || context.continuation_attempt() != ContinuationAttempt::Native
            || body.get("previous_response_id").and_then(Value::as_str)
                != Some(pin.upstream_response_id().as_str())
        {
            return Err(replay_required());
        }
    }
    if body.get("generate") == Some(&Value::Bool(false)) || body.contains_key("conversation") {
        return Err(error(
            ProviderErrorKind::Unsupported,
            UpstreamSendState::NotSent,
        ));
    }
    body.insert("model".into(), Value::String(model.as_str().to_owned()));
    body.insert("stream".into(), Value::Bool(true));
    Ok(body)
}
fn infrastructure() -> ProviderError {
    error(
        ProviderErrorKind::ProviderInfrastructureUnavailable,
        UpstreamSendState::NotSent,
    )
}
fn replay_required() -> ProviderError {
    error(
        ProviderErrorKind::ContinuationRecoveryRequired,
        UpstreamSendState::NotSent,
    )
    .with_continuation_failure(ContinuationFailure::HistoryUnavailable)
    .with_continuation_recovery_disposition(ContinuationRecoveryDisposition::ClientReplayRequired)
}
fn preflight(context: &AttemptContext) -> Result<(), ProviderError> {
    if context.cancellation().is_cancelled() {
        return Err(error(
            ProviderErrorKind::Cancelled,
            UpstreamSendState::NotSent,
        ));
    }
    if context.deadline().is_elapsed() {
        return Err(error(
            ProviderErrorKind::Timeout,
            UpstreamSendState::NotSent,
        ));
    }
    Ok(())
}
async fn read_error_body(
    response: &mut reqwest::Response,
    context: &AttemptContext,
) -> Result<Value, ProviderError> {
    let mut bytes = Vec::new();
    loop {
        let next = tokio::select! {
                    biased;
            _=context.cancellation().cancelled()=>return Err(error(ProviderErrorKind::Cancelled,UpstreamSendState::Sent)),
            _=context.deadline().wait()=>return Err(error(ProviderErrorKind::Timeout,UpstreamSendState::Sent)),
            next=response.chunk()=>next,
        };
        let Some(next) =
            next.map_err(|_| error(ProviderErrorKind::Transport, UpstreamSendState::Sent))?
        else {
            break;
        };
        if bytes.len().saturating_add(next.len()) > 256 * 1024 {
            return Ok(Value::Null);
        }
        bytes.extend_from_slice(&next);
    }
    Ok(serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}
