//! 执行会话跨重试共享 New API Token 准入结果

use std::num::NonZeroU32;

use gateway_core::error::TransientRetryPolicy;
use gateway_core::policy::{
    CallerIdentityPolicy, ForwardedCallerHeaders, NewApiTokenAffinityConfig,
};

use super::*;
use crate::engine::coordinator::{Script, ScriptedProvider};

#[test]
fn retries_share_admission_but_independent_executions_do_not() {
    block_on(async {
        let provider_kind = ProviderKind::new("openai").unwrap();
        let capabilities =
            ModelCapabilities::new(BTreeSet::from([OperationKind::Generate]), Some(16_000));
        let source_id = "12345678-1234-1234-1234-123456789abc";
        let policy = ClientPolicy::new(
            ClientApiKeyId::new("key_start_test").unwrap(),
            PlaintextClientApiKey::new("sk_start_test").unwrap(),
            account_scope(&provider_kind, "acct_start"),
            true,
            RateLimits::unlimited(),
        )
        .with_identity_policy(CallerIdentityPolicy::NewApi {
            source_id: source_id.to_owned(),
            allow_unidentified: false,
        });
        let snapshot = RuntimeSnapshot::new(
            ConfigRevision::new(1).unwrap(),
            gateway_core::settings::SettingsValues::new(1, "smart", Default::default(), None, None),
            vec![provider_kind.clone()],
            vec![ProviderModel::new(
                provider_kind,
                UpstreamModelId::new("gpt-start").unwrap(),
                capabilities,
            )],
            vec![policy],
        )
        .unwrap()
        .with_newapi_token_affinity(
            NewApiTokenAffinityConfig {
                enabled: true,
                request_limit: 1,
                target_account: Some("acct_start".to_owned()),
            },
            BTreeMap::new(),
        );
        let retry = || Script::Stream {
            account_id: "acct_start",
            items: vec![Err(ProviderError::new(
                ProviderErrorKind::Unavailable,
                UpstreamSendState::Sent,
            )
            .with_status(503)
            .with_replay_safe()
            .with_transient_retry(
                NonZeroU32::MIN,
                Duration::ZERO,
                Duration::ZERO,
                Duration::ZERO,
                TransientRetryPolicy::RetryThenReturnError,
            ))],
        };
        let stop = || {
            Script::Error(ProviderError::new(
                ProviderErrorKind::NoEligibleAccount,
                UpstreamSendState::NotSent,
            ))
        };
        let provider = Arc::new(ScriptedProvider::new(vec![
            retry(),
            stop(),
            retry(),
            stop(),
        ]));
        let service = DefaultExecutionService::new(
            RuntimeSnapshotHandle::new(snapshot),
            Arc::new(TrackingExecutionStore::default()),
            ProviderRegistry::new([provider.clone() as Arc<dyn Provider>]).unwrap(),
            Arc::new(Admissions::default()),
            Arc::new(UnusedContinuation),
            Arc::new(RecordingClientApiKeyUsage::default()),
            Arc::new(crate::RecordingDiagnostics::default()),
        );

        for _ in 0..2 {
            let authentication = ClientAuthenticationRequest::bearer("sk_start_test")
                .unwrap()
                .with_forwarded_caller(ForwardedCallerHeaders {
                    users: vec!["1".to_owned()],
                    tokens: vec!["2".to_owned()],
                    ..Default::default()
                });
            let client = service.authenticate_request(authentication).await.unwrap();
            let mut execution_request = request(&service, ClientTransport::HttpJson);
            execution_request.client = client;
            let mut started = service.start(execution_request).await.unwrap();
            assert!(started.session.collect_uncommitted().await.is_err());
        }

        let contexts = provider.contexts.lock().unwrap();
        assert_eq!(contexts.len(), 4);
        assert!(
            contexts
                .iter()
                .all(|context| context.deadline().at().is_none())
        );
        assert!(
            contexts
                .iter()
                .all(|context| context.newapi_token_affinity_rule().is_some())
        );
        assert!(
            contexts
                .iter()
                .all(|context| context.newapi_token_affinity_admission().is_none())
        );
        contexts[0].record_newapi_token_affinity_admission(true);
        assert_eq!(contexts[1].newapi_token_affinity_admission(), Some(true));
        assert_eq!(contexts[2].newapi_token_affinity_admission(), None);
        contexts[2].record_newapi_token_affinity_admission(false);
        contexts[3].record_newapi_token_affinity_admission(true);
        assert_eq!(contexts[3].newapi_token_affinity_admission(), Some(false));
        assert_eq!(contexts[0].newapi_token_affinity_admission(), Some(true));
    });
}

#[test]
fn switched_admission_preserves_same_account_retry_without_provider_fallback() {
    for scenario in 0..3 {
        block_on(async {
            let bounded = scenario == 2;
            let openai = ProviderKind::new("openai").unwrap();
            let xai = ProviderKind::new("xai").unwrap();
            let directory = Arc::new(RuntimeAccountDirectory::new(BTreeMap::from([
                (
                    ProviderAccountId::new("acct_start").unwrap(),
                    RuntimeAccount::new(openai.clone(), BTreeSet::new()),
                ),
                (
                    ProviderAccountId::new("acct_xai").unwrap(),
                    RuntimeAccount::new(xai.clone(), BTreeSet::new()),
                ),
            ])));
            let policy = ClientPolicy::new(
                ClientApiKeyId::new("key_start_test").unwrap(),
                PlaintextClientApiKey::new("sk_start_test").unwrap(),
                Arc::new(FrozenAccountScope::new(
                    Arc::clone(&directory),
                    ClientRoutingScope::all_accounts(),
                )),
                true,
                RateLimits::unlimited(),
            )
            .with_identity_policy(CallerIdentityPolicy::NewApi {
                source_id: "12345678-1234-1234-1234-123456789abc".to_owned(),
                allow_unidentified: false,
            });
            let capabilities =
                ModelCapabilities::new(BTreeSet::from([OperationKind::Generate]), Some(16_000));
            let snapshot = RuntimeSnapshot::new(
                ConfigRevision::new(1).unwrap(),
                gateway_core::settings::SettingsValues::new(
                    2,
                    "smart",
                    Default::default(),
                    None,
                    None,
                )
                .with_recovery_attempts(2)
                .with_recovery_rotation_attempts(0),
                vec![openai.clone(), xai.clone()],
                vec![openai, xai]
                    .into_iter()
                    .map(|provider| {
                        ProviderModel::new(
                            provider,
                            UpstreamModelId::new("gpt-start").unwrap(),
                            capabilities.clone(),
                        )
                    })
                    .collect(),
                vec![policy],
            )
            .unwrap()
            .with_account_directory(directory)
            .with_newapi_token_affinity(
                NewApiTokenAffinityConfig {
                    enabled: true,
                    request_limit: 1,
                    target_account: Some("acct_start".to_owned()),
                },
                BTreeMap::new(),
            );
            let retry_policy = if bounded {
                TransientRetryPolicy::RetryThenRotateAccountWithBudget {
                    max_total_retries: NonZeroU32::new(2).unwrap(),
                    track_client_retry_failure: false,
                }
            } else {
                TransientRetryPolicy::RetryThenRotateAccount
            };
            let failure = || Script::Stream {
                account_id: "acct_start",
                items: vec![Err(ProviderError::new(
                    ProviderErrorKind::Unavailable,
                    UpstreamSendState::Sent,
                )
                .with_replay_safe()
                .with_transient_retry(
                    NonZeroU32::MIN,
                    Duration::ZERO,
                    Duration::ZERO,
                    Duration::ZERO,
                    retry_policy,
                ))],
            };
            let second = if bounded {
                failure()
            } else {
                Script::Error(ProviderError::new(
                    if scenario == 0 {
                        ProviderErrorKind::NoEligibleAccount
                    } else {
                        ProviderErrorKind::AccountCapacityUnavailable
                    },
                    UpstreamSendState::NotSent,
                ))
            };
            let mut openai = ScriptedProvider::new(vec![failure(), second]);
            openai.newapi_admission = Some(true);
            let openai = Arc::new(openai);
            let xai = Arc::new(ScriptedProvider::named("xai", Vec::new()));
            let service = DefaultExecutionService::new(
                RuntimeSnapshotHandle::new(snapshot),
                Arc::new(TrackingExecutionStore::default()),
                ProviderRegistry::new([
                    openai.clone() as Arc<dyn Provider>,
                    xai.clone() as Arc<dyn Provider>,
                ])
                .unwrap(),
                Arc::new(Admissions::default()),
                Arc::new(UnusedContinuation),
                Arc::new(RecordingClientApiKeyUsage::default()),
                Arc::new(crate::RecordingDiagnostics::default()),
            );
            let client = service
                .authenticate_request(
                    ClientAuthenticationRequest::bearer("sk_start_test")
                        .unwrap()
                        .with_forwarded_caller(ForwardedCallerHeaders {
                            users: vec!["1".to_owned()],
                            tokens: vec!["2".to_owned()],
                            ..Default::default()
                        }),
                )
                .await
                .unwrap();
            let mut execution_request = request(&service, ClientTransport::HttpJson);
            execution_request.client = client;
            let mut started = service.start(execution_request).await.unwrap();
            assert!(started.session.collect_uncommitted().await.is_err());
            let contexts = openai.contexts.lock().unwrap();
            assert_eq!(contexts.len(), 2);
            assert_eq!(
                contexts[1].required_account().unwrap().as_str(),
                "acct_start"
            );
            assert!(xai.contexts.lock().unwrap().is_empty());
        });
    }
}
