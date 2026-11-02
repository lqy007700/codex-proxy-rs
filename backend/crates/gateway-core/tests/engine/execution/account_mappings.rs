//! 验证账号映射目录的原生对象优先级、条目顺序与来源隔离

use super::*;
use gateway_core::account::AccountModelMappings;
use gateway_core::routing::{ProviderModelContent, ProviderModelDescriptor, PublicModelDescriptor};

struct AccountCatalogProvider;

#[async_trait]
impl Provider for AccountCatalogProvider {
    fn name(&self) -> &'static str {
        "openai"
    }

    fn catalog_generation(&self) -> ProviderCatalogGeneration {
        ProviderCatalogGeneration::default()
    }

    async fn query_model_capabilities(
        &self,
    ) -> Result<Vec<ProviderModelCapabilities>, ProviderError> {
        Ok(Vec::new())
    }

    async fn query_client_model_catalog(
        &self,
        _: &FrozenAccountScope,
        _: &str,
        _: &str,
    ) -> Result<
        Option<Vec<ProviderModelDescriptor>>,
        gateway_core::routing::ProviderCatalogUnavailable,
    > {
        Ok(Some(
            [
                ("acct_other", "private-target", true),
                ("acct_a", "a-target", false),
                ("acct_a", "z-target", false),
                ("acct_b", "z-target", true),
                ("acct_b", "a-target", true),
            ]
            .into_iter()
            .map(|(account, model, native)| ProviderModelDescriptor {
                model: UpstreamModelId::new(model).unwrap(),
                source_account: Some(ProviderAccountId::new(account).unwrap()),
                content: if native {
                    ProviderModelContent::Native(
                        RawJsonPayload::new(
                            "codex",
                            serde_json::to_vec(&json!({"slug":model,"future":{"opaque":true}}))
                                .unwrap()
                                .into(),
                        )
                        .unwrap(),
                    )
                } else {
                    ProviderModelContent::Adapted(gateway_core::routing::ModelPresentation::new(
                        Some(format!("Simplified {model}")),
                        None,
                    ))
                },
            })
            .collect(),
        ))
    }

    async fn execute(
        self: Arc<Self>,
        _: ProviderRequest,
        _: AttemptContext,
    ) -> Result<ProviderStream, ProviderError> {
        panic!("目录查询不能创建推理 attempt")
    }
}

fn mapped_client_catalog() -> Vec<PublicModelDescriptor> {
    let provider = ProviderKind::new("openai").unwrap();
    let directory = Arc::new(RuntimeAccountDirectory::new(
        ["acct_a", "acct_b", "acct_other"]
            .into_iter()
            .map(|id| {
                (
                    ProviderAccountId::new(id).unwrap(),
                    RuntimeAccount::new(provider.clone(), BTreeSet::new()).with_model_mappings(
                        AccountModelMappings::new(BTreeMap::from([(
                            "alias".to_owned(),
                            "z-target".to_owned(),
                        )]))
                        .unwrap(),
                    ),
                )
            })
            .collect(),
    ));
    let scope = Arc::new(
        FrozenAccountScope::new(directory.clone(), ClientRoutingScope::all_accounts())
            .restricted_to(
                &Default::default(),
                &BTreeSet::from([
                    ProviderAccountId::new("acct_a").unwrap(),
                    ProviderAccountId::new("acct_b").unwrap(),
                ]),
            ),
    );
    let snapshot = RuntimeSnapshot::new(
        ConfigRevision::new(1).unwrap(),
        start_snapshot().settings().clone(),
        vec![provider.clone()],
        ["a-target", "z-target", "private-target"]
            .into_iter()
            .map(|id| {
                ProviderModel::new(
                    provider.clone(),
                    UpstreamModelId::new(id).unwrap(),
                    ModelCapabilities::new(BTreeSet::from([OperationKind::Generate]), None),
                )
            })
            .collect(),
        vec![ClientPolicy::new(
            ClientApiKeyId::new("key_start_test").unwrap(),
            PlaintextClientApiKey::new("sk_start_test").unwrap(),
            scope,
            true,
            RateLimits::unlimited(),
        )],
    )
    .unwrap()
    .with_account_directory(directory);
    let service = DefaultExecutionService::new(
        RuntimeSnapshotHandle::new(snapshot),
        Arc::new(TrackingExecutionStore::default()),
        ProviderRegistry::new([Arc::new(AccountCatalogProvider) as Arc<dyn Provider>]).unwrap(),
        Arc::new(UnusedAdmissions),
        Arc::new(UnusedContinuation),
        Arc::new(RecordingClientApiKeyUsage::default()),
        Arc::new(crate::RecordingDiagnostics::default()),
    );
    let client = service.authenticate("sk_start_test").unwrap();
    block_on(service.client_model_catalog(&client, "codex", "0.154.0")).unwrap()
}

#[test]
fn account_mapping_catalog_preserves_native_order_and_omits_unauthorized_sources() {
    let models = mapped_client_catalog();
    let ids = models
        .iter()
        .map(|entry| match entry {
            PublicModelDescriptor::Native { model, .. } => model.as_str(),
            PublicModelDescriptor::Adapted(profile) => profile.model().as_str(),
        })
        .collect::<Vec<_>>();
    assert_eq!(ids, ["z-target", "a-target", "alias"]);
}

#[test]
fn account_mapping_catalog_prefers_complete_native_objects_and_keeps_alias_payload_intact() {
    let models = mapped_client_catalog();
    let native = |name: &str| {
        models
            .iter()
            .find_map(|entry| match entry {
                PublicModelDescriptor::Native { model, payload } if model.as_str() == name => {
                    Some(payload.body())
                }
                _ => None,
            })
            .expect("同一目标有完整原生对象时不能退回简化画像")
    };
    assert_eq!(native("alias"), native("z-target"));
    assert!(
        serde_json::from_slice::<Value>(native("a-target")).unwrap()["future"]["opaque"]
            .as_bool()
            .unwrap()
    );
}
