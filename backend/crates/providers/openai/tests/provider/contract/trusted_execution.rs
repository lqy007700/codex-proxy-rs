//! 可信转发身份经真实认证、执行协调与 Provider 的集成测试辅助

use super::*;
use async_trait::async_trait;
use futures::future::BoxFuture;
use gateway_core::engine::{
    admission::*,
    continuation::*,
    execution::*,
    provider::{Provider, ProviderRegistry},
};
use gateway_core::policy::{
    CallerIdentityPolicy, ClientPolicy, ForwardedCallerHeaders, PlaintextClientApiKey, RateLimits,
};
use gateway_core::runtime::RuntimeSnapshotHandle;

struct Ports;
impl ClientAdmissionPort for Ports {
    fn abandon(&self, _: &ClientApiKeyId, _: &ModelRequestId) {}
    fn admit(
        &self,
        _: ClientAdmissionRequest,
    ) -> BoxFuture<'_, Result<ClientAdmissionDecision, ClientAdmissionError>> {
        Box::pin(async { Ok(ClientAdmissionDecision::Granted) })
    }
    fn release<'a>(
        &'a self,
        _: &'a ClientApiKeyId,
        _: &'a ModelRequestId,
    ) -> BoxFuture<'a, Result<bool, ClientAdmissionError>> {
        Box::pin(async { Ok(true) })
    }
    fn restore(
        &self,
        _: ClientAdmissionRecovery,
    ) -> BoxFuture<'_, Result<ClientAdmissionRestoreResult, ClientAdmissionError>> {
        Box::pin(async { Ok(ClientAdmissionRestoreResult::default()) })
    }
}
impl NativeContinuationPort for Ports {
    fn resolve<'a>(
        &'a self,
        _: &'a ClientApiKeyId,
        _: &'a PreviousResponseId,
    ) -> BoxFuture<'a, Result<Option<NativeContinuationPin>, NativeContinuationStoreError>> {
        Box::pin(async { Ok(None) })
    }
    fn record<'a>(
        &'a self,
        _: NativeContinuationPin,
    ) -> BoxFuture<'a, Result<(), NativeContinuationStoreError>> {
        Box::pin(async { Ok(()) })
    }
}
impl ClientApiKeyUsageSink for Ports {
    fn record_used(&self, _: &ClientApiKeyId) {}
}
#[async_trait]
impl gateway_core::diagnostics::OperationalDiagnostics for Ports {
    async fn record_failure(
        &self,
        _: gateway_core::diagnostics::OperationalFailure,
    ) -> Result<(), gateway_core::error::StoreError> {
        Ok(())
    }
}

pub(super) async fn start(
    provider: Arc<CodexProvider>,
    operation: Operation,
    settings: SettingsValues,
    scope: Arc<FrozenAccountScope>,
    client_key: &str,
) -> Box<dyn ExecutionSession> {
    let kind = ProviderKind::new("openai").unwrap();
    let snapshot = RuntimeSnapshot::new(
        ConfigRevision::new(1).unwrap(),
        settings,
        vec![kind.clone()],
        vec![ProviderModel::new(
            kind,
            UpstreamModelId::new("gpt-5.4").unwrap(),
            ModelCapabilities::new(BTreeSet::from([operation.kind()]), Some(32_000))
                .with_upstream_feature_validation(),
        )],
        vec![
            ClientPolicy::new(
                ClientApiKeyId::new(client_key).unwrap(),
                PlaintextClientApiKey::new("sk_fixture_trusted").unwrap(),
                scope,
                true,
                RateLimits::unlimited(),
            )
            .with_identity_policy(CallerIdentityPolicy::NewApi {
                source_id: "b7bb7de9-7b06-4caa-8070-352af86a8a4a".into(),
                allow_unidentified: false,
            }),
        ],
    )
    .unwrap();
    let service = DefaultExecutionService::new(
        RuntimeSnapshotHandle::new(snapshot),
        Arc::new(super::selection_retry::NoopExecutionStore),
        ProviderRegistry::new([provider as Arc<dyn Provider>]).unwrap(),
        Arc::new(Ports),
        Arc::new(Ports),
        Arc::new(Ports),
        Arc::new(Ports),
    );
    let client = service
        .authenticate_request(
            gateway_core::engine::authentication::ClientAuthenticationRequest::bearer(
                "sk_fixture_trusted",
            )
            .unwrap()
            .with_forwarded_caller(ForwardedCallerHeaders {
                users: vec!["1".into()],
                tokens: vec!["11".into()],
            }),
        )
        .await
        .unwrap();
    assert!(matches!(
        client.policy().caller_identity(),
        gateway_core::policy::CallerIdentity::NewApi { .. }
    ));
    service
        .start(StartExecution {
            client,
            public_model: PublicModelId::new("gpt-5.4").unwrap(),
            operation,
            metadata: ExecutionRequestMetadata {
                protocol: "openai".into(),
                endpoint: "responses".into(),
                transport: ClientTransport::HttpSse,
                stream: true,
                client_ip: None,
                user_agent: None,
                previous_response_id: None,
            },
        })
        .await
        .unwrap()
        .session
}

#[tokio::test]
async fn trusted_token_keeps_primary_across_same_account_retry_rotation_and_fallback() {
    use gateway_core::account::AccountSchedulingRole;
    use gateway_core::provider_ports::ProviderAffinityScope;
    use std::sync::atomic::{AtomicBool, Ordering};
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_provider_contract").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let fail = Arc::new(AtomicBool::new(false));
    let server = MockServer::start().await;
    let failed = fail.clone();
    Mock::given(method("POST"))
        .respond_with(move |_: &wiremock::Request| {
            if failed.load(Ordering::SeqCst) {
                ResponseTemplate::new(503).set_body_json(
                    json!({"error":{"type":"server_error","message":"fixture busy"}}),
                )
            } else {
                ResponseTemplate::new(200).set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream")
            }
        })
        .mount(&server)
        .await;
    let provider = provider_with_affinity_and_base_url(&store, affinity.clone(), server.uri());
    let settings = || {
        SettingsValues::new(2, "smart", Default::default(), None, None)
            .with_request_location(global_request_location(), true)
            .with_recovery_attempts(2)
            .with_recovery_rotation_attempts(1)
    };
    start(
        provider.clone(),
        http_generate_operation(),
        settings(),
        contract_account_scope(),
        "key_shared",
    )
    .await
    .collect_uncommitted()
    .await
    .unwrap();
    for id in ["acct_subagent_a", "acct_subagent_b"] {
        create_account(&store, id).await;
    }
    store.set_scheduling_role("acct_subagent_b", AccountSchedulingRole::Fallback);
    let kind = ProviderKind::new("openai").unwrap();
    let scope = Arc::new(FrozenAccountScope::new(
        Arc::new(RuntimeAccountDirectory::new(
            [
                "acct_provider_contract",
                "acct_subagent_a",
                "acct_subagent_b",
            ]
            .into_iter()
            .map(|id| {
                (
                    ProviderAccountId::new(id).unwrap(),
                    RuntimeAccount::new(kind.clone(), BTreeSet::new()).with_scheduling_role(
                        if id == "acct_subagent_b" {
                            AccountSchedulingRole::Fallback
                        } else {
                            AccountSchedulingRole::Regular
                        },
                    ),
                )
            })
            .collect(),
        )),
        ClientRoutingScope::all_accounts(),
    ));
    fail.store(true, Ordering::SeqCst);
    let mut session = start(
        provider,
        http_generate_operation(),
        settings(),
        scope,
        "key_rotated",
    )
    .await;
    assert!(session.collect_uncommitted().await.is_err());
    let requests = server.received_requests().await.unwrap();
    let accounts: Vec<_> = requests
        .iter()
        .map(|r| r.headers["authorization"].to_str().unwrap())
        .collect();
    assert_eq!(
        accounts,
        [
            "Bearer at-acct_provider_contract",
            "Bearer at-acct_provider_contract",
            "Bearer at-acct_provider_contract",
            "Bearer at-acct_subagent_a",
            "Bearer at-acct_subagent_b"
        ]
    );
    assert_eq!(
        affinity
            .binding_for_scope(ProviderAffinityScope::Token)
            .1
            .account_id()
            .as_str(),
        "acct_provider_contract"
    );
}
