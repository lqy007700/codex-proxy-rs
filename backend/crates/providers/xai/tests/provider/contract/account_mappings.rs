//! 验证 xAI 原生选号使用账号目标政策并发送相同的实际模型

use super::*;
use gateway_core::account::{AccountModelAccess, AccountModelAccessMode, AccountModelMappings};

#[tokio::test]
async fn account_mapping_uses_actual_target_for_policy_selection_and_wire() {
    let store = MemoryProviderAccountStore::shared();
    let repository = GrokCredentialRepository::new(store.clone());
    let kind = ProviderKind::new("xai").unwrap();
    let mut accounts = BTreeMap::new();
    for (suffix, target) in [("mapped-a", "grok-4.5"), ("mapped-b", "grok-4.7")] {
        let input = create_input(suffix, suffix);
        seed_input(&store, &input).await.unwrap();
        repository
            .update_state(&UpdateGrokCredentialState {
                account_id: input.account_id.clone(),
                expected_revision: CredentialRevision::new(1).unwrap(),
                credential_state: CredentialState::Ready,
                error_reason: None,
                error_message: None,
                observed_at: chrono::Utc::now(),
            })
            .await
            .unwrap();
        accounts.insert(
            input.account_id,
            RuntimeAccount::new(kind.clone(), BTreeSet::new())
                .with_model_mappings(
                    AccountModelMappings::new(BTreeMap::from([(
                        "client-model".to_owned(),
                        target.to_owned(),
                    )]))
                    .unwrap(),
                )
                .with_model_access(
                    AccountModelAccess::new(
                        AccountModelAccessMode::Allowlist,
                        vec![target.to_owned()],
                    )
                    .unwrap(),
                ),
        );
    }
    let cache = MemoryGrokCatalogCache::shared();
    let selector = GrokAccountSessionSelector::new(
        kind.clone(),
        repository.clone(),
        cache.clone(),
        Arc::new(crate::support::grok_quota_service(
            repository.clone(),
            Arc::new(UnavailableBillingTransport),
        )),
        Arc::new(DiagnosticLeasePort),
        Arc::new(MemoryCooldownPort::default()),
        Arc::new(AccountFeedbackStats::default()),
    );
    let transport = StubInferenceTransport::success();
    let provider = Arc::new(
        GrokBuildProvider::new(
            Arc::new(selector),
            transport.clone(),
            Arc::new(crate::support::grok_catalog_service(
                repository,
                Arc::new(StaticCatalogTransport),
                cache,
            )),
            StubRecovery::new(GrokCredentialRecoveryOutcome::Unavailable),
            Arc::new(AccountFeedbackStats::default()),
            crate::support::xai_wire_profile(),
        )
        .unwrap(),
    );
    let operation = operation();
    let snapshot = RuntimeSnapshot::new(
        ConfigRevision::new(1).unwrap(),
        gateway_core::settings::SettingsValues::new(2, "smart", Default::default(), None, None),
        vec![kind.clone()],
        ["grok-4.5", "grok-4.7"]
            .into_iter()
            .map(|name| {
                ProviderModel::new(
                    kind.clone(),
                    UpstreamModelId::new(name).unwrap(),
                    ModelCapabilities::new(BTreeSet::from([OperationKind::Generate]), Some(32_000))
                        .with_upstream_feature_validation(),
                )
            })
            .collect(),
        Vec::new(),
    )
    .unwrap()
    .with_account_directory(Arc::new(RuntimeAccountDirectory::new(accounts)));
    let public = PublicModelId::new("client-model").unwrap();
    let plan = snapshot
        .plan(
            &public,
            &operation,
            snapshot.all_account_scope(),
            &Default::default(),
        )
        .unwrap();
    let candidate = plan.candidates()[0].clone();
    let context = AttemptContext::new(
        gateway_core::engine::RequestAttemptContext::new(
            ModelRequestId::new("req_xai_mapping").unwrap(),
            ClientApiKeyId::new("key_xai_contract").unwrap(),
        )
        .with_requested_model(Some(public)),
        NonZeroU32::MIN,
        SystemTime::now() + Duration::from_secs(30),
        selection_policy(),
        AccountAttemptContext::new(BTreeSet::new(), Some(account_id("mapped-b")), None)
            .with_routing_candidate(candidate.clone()),
        None,
        CancellationToken::new(),
    );
    let mut stream = provider
        .execute(ProviderRequest::new(operation, candidate), context)
        .await
        .unwrap();
    assert_eq!(
        stream.metadata().provider_account_id(),
        &account_id("mapped-b")
    );
    assert_eq!(
        stream.metadata().upstream_model().unwrap().as_str(),
        "grok-4.7"
    );
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
    let requests = transport.requests.lock().unwrap();
    let sent: Value = serde_json::from_slice(requests[0].body()).unwrap();
    assert_eq!(sent["model"], "grok-4.7");
}
