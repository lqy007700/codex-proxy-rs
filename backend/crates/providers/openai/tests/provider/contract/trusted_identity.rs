//! 模拟共享 CPR Key 的多用户和多 Token，验证真实 Provider 选号与管理指令

use super::session_binding::turn_request;
use super::*;
use gateway_core::policy::CallerIdentity;
use gateway_core::provider_ports::SessionBindingDirective;

fn caller(source: &str, user: &str, token: &str) -> CallerIdentity {
    CallerIdentity::NewApi {
        source_id: source.into(),
        user_id: user.into(),
        token_id: token.into(),
    }
}
fn scoped_context(id: &str, key: &str, caller: CallerIdentity) -> AttemptContext {
    scoped_recovery_context(
        id,
        key,
        caller,
        gateway_core::engine::recovery::AccountRecoveryStage::Initial,
    )
}
fn scoped_recovery_context(
    id: &str,
    key: &str,
    caller: CallerIdentity,
    stage: gateway_core::engine::recovery::AccountRecoveryStage,
) -> AttemptContext {
    AttemptContext::new(
        RequestAttemptContext::new(
            ModelRequestId::new(id).unwrap(),
            ClientApiKeyId::new(key).unwrap(),
        )
        .with_caller_identity(caller)
        .with_request_location(Some(global_request_location())),
        NonZeroU32::MIN,
        SystemTime::now() + Duration::from_secs(10),
        account_policy(),
        AccountAttemptContext::new(BTreeSet::new(), None, None)
            .with_account_scope(contract_account_scope())
            .with_recovery_stage(stage),
        None,
        CancellationToken::new(),
    )
}
#[tokio::test]
async fn shared_key_callers_isolate_bindings_and_turn_aliases_while_rotated_key_reuses_source_scope()
 {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_subagent_a").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream"),
        )
        .expect(6)
        .mount(&server)
        .await;
    let provider = provider_with_affinity_and_base_url(&store, affinity.clone(), server.uri());
    let scopes = [
        (
            "key_shared",
            CallerIdentity::Direct {
                key_id: "key_shared".into(),
            },
            1,
        ),
        ("key_shared", caller("source_a", "1", "11"), 2),
        ("key_rotated", caller("source_a", "1", "11"), 2),
        ("key_shared", caller("source_a", "1", "12"), 3),
        ("key_shared", caller("source_a", "2", "11"), 4),
        ("key_shared", caller("source_b", "1", "11"), 5),
    ];
    for (index, (key, caller, count)) in scopes.into_iter().enumerate() {
        let mut stream = provider
            .clone()
            .execute(
                planned_request("openai", turn_request("same-session", "same-turn")),
                scoped_context(&format!("req_caller_{index}"), key, caller),
            )
            .await
            .unwrap();
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
        assert_eq!(affinity.binding_count(), count);
    }
    assert_eq!(affinity.alias_ttls().len(), 6);
    server.verify().await;
}
#[tokio::test]
async fn unidentified_requests_never_create_persistent_binding_or_turn_alias() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_subagent_a").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let server = MockServer::start().await;
    let provider = provider_with_affinity_and_base_url(&store, affinity.clone(), server.uri());
    let stream = provider
        .execute(
            planned_request("openai", turn_request("same-session", "same-turn")),
            scoped_context(
                "req_probe",
                "key_shared",
                CallerIdentity::Unidentified {
                    request_id: "probe".into(),
                },
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        stream.metadata().provider_account_id().as_str(),
        "acct_subagent_a"
    );
    assert_eq!(affinity.binding_count(), 0);
    assert!(affinity.alias_ttls().is_empty());
}
#[tokio::test]
async fn management_reassign_and_target_are_consumed_only_by_safe_selection() {
    for recovering in [false, true] {
        for directive in [
            SessionBindingDirective::Clear,
            SessionBindingDirective::Reassign,
            SessionBindingDirective::Target {
                account_id: "acct_subagent_b".into(),
            },
        ] {
            let store = Arc::new(MemoryAccountStore::default());
            create_account(&store, "acct_subagent_a").await;
            let affinity = Arc::new(MemorySessionAffinity::default());
            let server = MockServer::start().await;
            let provider =
                provider_with_affinity_and_base_url(&store, affinity.clone(), server.uri());
            let request =
                || planned_request("openai", turn_request("managed-session", "managed-turn"));
            drop(
                provider
                    .clone()
                    .execute(
                        request(),
                        scoped_context("req_before", "key_shared", caller("source_a", "1", "11")),
                    )
                    .await
                    .unwrap(),
            );
            let (binding_key, old) = affinity.bindings_snapshot().pop().unwrap();
            create_account(&store, "acct_subagent_b").await;
            affinity.set_directive(&binding_key, directive.clone());
            let selected = provider
                .execute(
                    request(),
                    scoped_recovery_context(
                        "req_after",
                        "key_shared",
                        caller("source_a", "1", "11"),
                        if recovering {
                            gateway_core::engine::recovery::AccountRecoveryStage::OtherAccounts {
                                initial: ProviderAccountId::new("acct_subagent_a").unwrap(),
                                after: None,
                            }
                        } else {
                            gateway_core::engine::recovery::AccountRecoveryStage::Initial
                        },
                    ),
                )
                .await
                .unwrap();
            if !matches!(directive, SessionBindingDirective::Clear) {
                assert_eq!(
                    selected.metadata().provider_account_id().as_str(),
                    "acct_subagent_b"
                );
            }
            let (_, current) = affinity.bindings_snapshot().pop().unwrap();
            assert!(current.directive().is_none());
            assert_ne!(old.revision(), current.revision());
        }
    }
}

#[tokio::test]
async fn pending_management_never_rebinds_a_native_continuation_owner() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_subagent_a").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let server = MockServer::start().await;
    let provider = provider_with_affinity_and_base_url(&store, affinity.clone(), server.uri());
    drop(
        provider
            .clone()
            .execute(
                planned_request("openai", turn_request("root", "managed-turn")),
                context("req_managed_root", CancellationToken::new()),
            )
            .await
            .unwrap(),
    );
    create_account(&store, "acct_subagent_b").await;
    let (key, _) = affinity.bindings_snapshot().pop().unwrap();
    affinity.set_directive(
        &key,
        SessionBindingDirective::Target {
            account_id: "acct_subagent_b".into(),
        },
    );
    let error = provider
        .execute(
            planned_request(
                "openai",
                Operation::Generate(generate_with_session_context("root", Some("child"), None)),
            ),
            pinned_continuation_context(
                "req_managed_native",
                "acct_subagent_a",
                "resp_old",
                "resp_old",
                1,
                ContinuationAttempt::Native,
            ),
        )
        .await
        .err()
        .expect("pending native continuation must request full history");
    assert_eq!(
        error.kind(),
        ProviderErrorKind::ContinuationRecoveryRequired
    );
    let (_, binding) = affinity.bindings_snapshot().pop().unwrap();
    assert!(binding.directive().is_some());
    assert_eq!(binding.account_id().as_str(), "acct_subagent_a");
    assert!(server.received_requests().await.unwrap().is_empty());
}
