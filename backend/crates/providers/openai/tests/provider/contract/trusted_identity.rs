//! 模拟共享 CPR Key 的多用户和多 Token，验证真实 Provider 选号与管理指令

use super::session_binding::turn_request;
use super::*;
use gateway_core::policy::CallerIdentity;
use gateway_core::provider_ports::{ProviderAffinityScope, SessionBindingDirective};

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

fn token_context(scope: Arc<FrozenAccountScope>) -> AttemptContext {
    AttemptContext::new(
        RequestAttemptContext::new(
            ModelRequestId::new("req_token_affinity").unwrap(),
            ClientApiKeyId::new("key_shared").unwrap(),
        )
        .with_caller_identity(caller("source_a", "1", "11"))
        .with_request_location(Some(global_request_location())),
        NonZeroU32::MIN,
        SystemTime::now() + Duration::from_secs(10),
        account_policy(),
        AccountAttemptContext::new(BTreeSet::new(), None, None).with_account_scope(scope),
        None,
        CancellationToken::new(),
    )
}

fn primary(affinity: &MemorySessionAffinity) -> String {
    affinity
        .binding_for_scope(ProviderAffinityScope::Token)
        .1
        .account_id()
        .as_str()
        .to_owned()
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
        ("key_shared", caller("source_a", "1", "11"), 3),
        ("key_rotated", caller("source_a", "1", "11"), 3),
        ("key_shared", caller("source_a", "1", "12"), 5),
        ("key_shared", caller("source_a", "2", "11"), 7),
        ("key_shared", caller("source_b", "1", "11"), 9),
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
            let (binding_key, old) = affinity.binding_for_scope(ProviderAffinityScope::Session);
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
            let (_, current) = affinity.binding_for_scope(ProviderAffinityScope::Session);
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

#[tokio::test]
async fn token_primary_is_shared_across_windows_and_requests_without_session_ids() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_subagent_a").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let leases = Arc::new(TestLeaseCoordinator::default());
    let server = MockServer::start().await;
    let provider = provider_with_affinity_and_base_url_and_leases(
        &store,
        affinity.clone(),
        server.uri(),
        leases.clone(),
    );
    drop(
        provider
            .clone()
            .execute(
                planned_request("openai", turn_request("window-1", "turn-1")),
                token_context(contract_account_scope()),
            )
            .await
            .unwrap(),
    );
    create_account(&store, "acct_subagent_b").await;
    let original = affinity.binding_for_scope(ProviderAffinityScope::Token).1;
    for operation in [turn_request("window-2", "turn-2"), generate_operation()] {
        let stream = provider
            .clone()
            .execute(
                planned_request("openai", operation),
                token_context(contract_account_scope()),
            )
            .await
            .unwrap();
        assert_eq!(
            stream.metadata().provider_account_id().as_str(),
            "acct_subagent_a"
        );
    }
    assert_eq!(
        affinity.binding_for_scope(ProviderAffinityScope::Token).1,
        original
    );
    assert_eq!(
        affinity.binding_count(),
        3,
        "one Token primary and two independent session bindings"
    );
}

#[tokio::test]
async fn unsupported_model_spills_over_without_rebinding_the_token_and_returns_to_primary() {
    use gateway_core::account::{AccountModelAccess, AccountModelAccessMode};
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_subagent_a").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream"),
        )
        .expect(3)
        .mount(&server)
        .await;
    let provider = provider_with_affinity_and_base_url(&store, affinity.clone(), server.uri());
    create_account(&store, "acct_subagent_b").await;
    let scope = Arc::new(FrozenAccountScope::new(
        Arc::new(RuntimeAccountDirectory::new(
            [
                ("acct_subagent_a", "gpt-5.4"),
                ("acct_subagent_b", "gpt-5.5"),
            ]
            .into_iter()
            .map(|(id, model)| {
                (
                    ProviderAccountId::new(id).unwrap(),
                    RuntimeAccount::new(ProviderKind::new("openai").unwrap(), BTreeSet::new())
                        .with_model_access(
                            AccountModelAccess::new(
                                AccountModelAccessMode::Allowlist,
                                vec![model.into()],
                            )
                            .unwrap(),
                        ),
                )
            })
            .collect(),
        )),
        ClientRoutingScope::all_accounts(),
    ));
    for (index, model, expected) in [
        (0, "gpt-5.4", "acct_subagent_a"),
        (1, "gpt-5.5", "acct_subagent_b"),
        (2, "gpt-5.4", "acct_subagent_a"),
    ] {
        let mut stream = provider
            .clone()
            .execute(
                planned_request_for_model(
                    "openai",
                    turn_request(&format!("window-{index}"), &format!("turn-{index}")),
                    model,
                ),
                token_context(scope.clone()),
            )
            .await
            .unwrap();
        assert_eq!(stream.metadata().provider_account_id().as_str(), expected);
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
        assert_eq!(primary(&affinity), "acct_subagent_a");
    }
    let requests = server.received_requests().await.unwrap();
    assert_eq!(captured_request_body(&requests[1])["model"], "gpt-5.5");
    server.verify().await;
}

#[tokio::test]
async fn busy_primary_spills_over_but_new_window_prefers_it_after_capacity_recovers() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_subagent_a").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let leases = Arc::new(TestLeaseCoordinator::default());
    let server = MockServer::start().await;
    let provider = provider_with_affinity_and_base_url_and_leases(
        &store,
        affinity.clone(),
        server.uri(),
        leases.clone(),
    );
    drop(
        provider
            .clone()
            .execute(
                planned_request("openai", generate_operation()),
                token_context(contract_account_scope()),
            )
            .await
            .unwrap(),
    );
    create_account(&store, "acct_subagent_b").await;
    leases
        .busy_accounts
        .lock()
        .unwrap()
        .insert(ProviderAccountId::new("acct_subagent_a").unwrap());
    let stream = provider
        .clone()
        .execute(
            planned_request("openai", turn_request("busy-window", "busy-turn")),
            token_context(contract_account_scope()),
        )
        .await
        .unwrap();
    assert_eq!(
        stream.metadata().provider_account_id().as_str(),
        "acct_subagent_b"
    );
    drop(stream);
    assert_eq!(primary(&affinity), "acct_subagent_a");
    leases.busy_accounts.lock().unwrap().clear();
    let stream = provider
        .execute(
            planned_request("openai", turn_request("new-window", "new-turn")),
            token_context(contract_account_scope()),
        )
        .await
        .unwrap();
    assert_eq!(
        stream.metadata().provider_account_id().as_str(),
        "acct_subagent_a"
    );
}

#[tokio::test]
async fn disabled_primary_migrates_only_at_safe_admission() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_subagent_a").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let server = MockServer::start().await;
    let provider = provider_with_affinity_and_base_url(&store, affinity.clone(), server.uri());
    drop(
        provider
            .clone()
            .execute(
                planned_request("openai", generate_operation()),
                token_context(contract_account_scope()),
            )
            .await
            .unwrap(),
    );
    create_account(&store, "acct_subagent_b").await;
    store
        .set_enabled(&ProviderAccountId::new("acct_subagent_a").unwrap(), false)
        .await
        .unwrap();
    let old = affinity.binding_for_scope(ProviderAffinityScope::Token).1;
    let stream = provider
        .execute(
            planned_request("openai", generate_operation()),
            token_context(contract_account_scope()),
        )
        .await
        .unwrap();
    assert_eq!(
        stream.metadata().provider_account_id().as_str(),
        "acct_subagent_b"
    );
    let new = affinity.binding_for_scope(ProviderAffinityScope::Token).1;
    assert_eq!(new.account_id().as_str(), "acct_subagent_b");
    assert_ne!(old.revision(), new.revision());
}

#[tokio::test]
async fn recovery_rotation_and_fallback_do_not_overwrite_token_primary() {
    use gateway_core::engine::recovery::AccountRecoveryStage;
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_subagent_a").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let server = MockServer::start().await;
    let provider = provider_with_affinity_and_base_url(&store, affinity.clone(), server.uri());
    drop(
        provider
            .clone()
            .execute(
                planned_request("openai", generate_operation()),
                token_context(contract_account_scope()),
            )
            .await
            .unwrap(),
    );
    create_account(&store, "acct_subagent_b").await;
    for stage in [
        AccountRecoveryStage::OtherAccounts {
            initial: ProviderAccountId::new("acct_subagent_a").unwrap(),
            after: None,
        },
        AccountRecoveryStage::Fallback,
    ] {
        let scope = if stage == AccountRecoveryStage::Fallback {
            store.set_scheduling_role(
                "acct_subagent_b",
                gateway_core::account::AccountSchedulingRole::Fallback,
            );
            Arc::new(FrozenAccountScope::new(
                Arc::new(RuntimeAccountDirectory::new(
                    ["acct_subagent_a", "acct_subagent_b"]
                        .into_iter()
                        .map(|id| {
                            (
                                ProviderAccountId::new(id).unwrap(),
                                RuntimeAccount::new(
                                    ProviderKind::new("openai").unwrap(),
                                    BTreeSet::new(),
                                )
                                .with_scheduling_role(
                                    if id == "acct_subagent_b" {
                                        gateway_core::account::AccountSchedulingRole::Fallback
                                    } else {
                                        gateway_core::account::AccountSchedulingRole::Regular
                                    },
                                ),
                            )
                        })
                        .collect(),
                )),
                ClientRoutingScope::all_accounts(),
            ))
        } else {
            contract_account_scope()
        };
        let attempt = AttemptContext::new(
            RequestAttemptContext::new(
                ModelRequestId::new("req_token_recovery").unwrap(),
                ClientApiKeyId::new("key_shared").unwrap(),
            )
            .with_caller_identity(caller("source_a", "1", "11"))
            .with_request_location(Some(global_request_location())),
            NonZeroU32::MIN,
            SystemTime::now() + Duration::from_secs(5),
            account_policy(),
            AccountAttemptContext::new(BTreeSet::new(), None, None)
                .with_account_scope(scope)
                .with_recovery_stage(stage),
            None,
            CancellationToken::new(),
        );
        let stream = provider
            .clone()
            .execute(planned_request("openai", generate_operation()), attempt)
            .await
            .unwrap();
        assert_eq!(
            stream.metadata().provider_account_id().as_str(),
            "acct_subagent_b"
        );
        assert_eq!(primary(&affinity), "acct_subagent_a");
    }
}

#[tokio::test]
async fn concurrent_first_windows_admit_one_token_primary_before_sending() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_subagent_a").await;
    create_account(&store, "acct_subagent_b").await;
    let affinity = Arc::new(MemorySessionAffinity::with_initial_claim_barrier(2));
    let server = MockServer::start().await;
    let provider = provider_with_affinity_and_base_url(&store, affinity.clone(), server.uri());
    let request = || {
        provider.clone().execute(
            planned_request("openai", generate_operation()),
            token_context(contract_account_scope()),
        )
    };
    let (a, b) = timeout(Duration::from_secs(5), async {
        tokio::join!(request(), request())
    })
    .await
    .unwrap();
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(
        a.metadata().provider_account_id(),
        b.metadata().provider_account_id()
    );
    assert_eq!(affinity.binding_count(), 1);
}

#[tokio::test]
async fn token_target_directive_changes_new_chains_but_does_not_move_native_owner() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_subagent_a").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let server = MockServer::start().await;
    let provider = provider_with_affinity_and_base_url(&store, affinity.clone(), server.uri());
    drop(
        provider
            .clone()
            .execute(
                planned_request("openai", turn_request("old-window", "old-turn")),
                token_context(contract_account_scope()),
            )
            .await
            .unwrap(),
    );
    create_account(&store, "acct_subagent_b").await;
    let (key, _) = affinity.binding_for_scope(ProviderAffinityScope::Token);
    affinity.set_directive(
        &key,
        SessionBindingDirective::Target {
            account_id: "acct_subagent_b".into(),
        },
    );
    let identity = caller("source_a", "1", "11");
    let owner = ProviderAccountStateOwner::new(
        ProviderKind::new("openai").unwrap(),
        ProviderAccountId::new("acct_subagent_a").unwrap(),
    );
    let pin = NativeContinuationPin::new(
        PreviousResponseId::new("resp_old"),
        PreviousResponseId::new("resp_old"),
        ClientApiKeyId::new("key_shared").unwrap(),
        ProviderKind::new("openai").unwrap(),
        ProviderAccountId::new("acct_subagent_a").unwrap(),
    )
    .with_caller_identity(identity.clone())
    .with_scope(gateway_core::engine::continuation::NativeContinuationScope::Persisted);
    let native = || {
        AttemptContext::new(
            RequestAttemptContext::new(
                ModelRequestId::new("req_old_owner").unwrap(),
                ClientApiKeyId::new("key_shared").unwrap(),
            )
            .with_caller_identity(identity.clone())
            .with_request_location(Some(global_request_location())),
            NonZeroU32::MIN,
            SystemTime::now() + Duration::from_secs(5),
            account_policy(),
            AccountAttemptContext::new(BTreeSet::new(), None, Some(owner.clone()))
                .with_account_scope(contract_account_scope()),
            Some(ContinuationBinding::Pinned(pin.clone())),
            CancellationToken::new(),
        )
        .with_continuation_attempt(ContinuationAttempt::Native)
    };
    for switched in [false, true] {
        if switched {
            let stream = provider
                .clone()
                .execute(
                    planned_request("openai", turn_request("new-window", "new-turn")),
                    token_context(contract_account_scope()),
                )
                .await
                .unwrap();
            assert_eq!(
                stream.metadata().provider_account_id().as_str(),
                "acct_subagent_b"
            );
        }
        let stream = provider
            .clone()
            .execute(
                planned_request("openai", turn_request("old-window", "old-turn")),
                native(),
            )
            .await
            .unwrap();
        assert_eq!(
            stream.metadata().provider_account_id().as_str(),
            "acct_subagent_a"
        );
        let binding = affinity.binding_for_scope(ProviderAffinityScope::Token).1;
        assert_eq!(
            binding.account_id().as_str(),
            if switched {
                "acct_subagent_b"
            } else {
                "acct_subagent_a"
            }
        );
        assert_eq!(binding.directive().is_some(), !switched);
    }
}

#[tokio::test]
async fn model_exhaustion_retains_primary_while_scope_removal_allows_safe_rebinding() {
    use gateway_core::account::{AccountModelAccess, AccountModelAccessMode};
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_subagent_a").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let provider = provider_with_affinity(&store, affinity.clone());
    drop(
        provider
            .clone()
            .execute(
                planned_request("openai", generate_operation()),
                token_context(contract_account_scope()),
            )
            .await
            .unwrap(),
    );
    create_account(&store, "acct_subagent_b").await;
    let primary_before = affinity.binding_for_scope(ProviderAffinityScope::Token).1;
    let scope = |deny_model: bool| {
        Arc::new(FrozenAccountScope::new(
            Arc::new(RuntimeAccountDirectory::new(
                (if deny_model {
                    vec!["acct_subagent_a", "acct_subagent_b"]
                } else {
                    vec!["acct_subagent_b"]
                })
                .into_iter()
                .map(|id| {
                    let account =
                        RuntimeAccount::new(ProviderKind::new("openai").unwrap(), BTreeSet::new());
                    (
                        ProviderAccountId::new(id).unwrap(),
                        if deny_model {
                            account.with_model_access(
                                AccountModelAccess::new(
                                    AccountModelAccessMode::Allowlist,
                                    vec!["gpt-5.5".into()],
                                )
                                .unwrap(),
                            )
                        } else {
                            account
                        },
                    )
                })
                .collect(),
            )),
            ClientRoutingScope::all_accounts(),
        ))
    };
    assert!(
        provider
            .clone()
            .execute(
                planned_request("openai", generate_operation()),
                token_context(scope(true))
            )
            .await
            .is_err()
    );
    let retained = affinity.binding_for_scope(ProviderAffinityScope::Token).1;
    assert_eq!(retained.account_id(), primary_before.account_id());
    assert_eq!(retained.revision(), primary_before.revision());
    let stream = provider
        .execute(
            planned_request("openai", generate_operation()),
            token_context(scope(false)),
        )
        .await
        .unwrap();
    assert_eq!(
        stream.metadata().provider_account_id().as_str(),
        "acct_subagent_b"
    );
    assert_eq!(primary(&affinity), "acct_subagent_b");
}

#[tokio::test]
async fn native_owner_keeps_spillover_account_without_claiming_or_overwriting_primary() {
    for existing_primary in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        let affinity = Arc::new(MemorySessionAffinity::default());
        let provider = provider_with_affinity(&store, affinity.clone());
        if existing_primary {
            create_account(&store, "acct_subagent_a").await;
            drop(
                provider
                    .clone()
                    .execute(
                        planned_request("openai", generate_operation()),
                        token_context(contract_account_scope()),
                    )
                    .await
                    .unwrap(),
            );
        }
        create_account(&store, "acct_subagent_b").await;
        let owner = ProviderAccountStateOwner::new(
            ProviderKind::new("openai").unwrap(),
            ProviderAccountId::new("acct_subagent_b").unwrap(),
        );
        let pin = NativeContinuationPin::new(
            PreviousResponseId::new("resp_native"),
            PreviousResponseId::new("resp_native"),
            ClientApiKeyId::new("key_shared").unwrap(),
            ProviderKind::new("openai").unwrap(),
            ProviderAccountId::new("acct_subagent_b").unwrap(),
        )
        .with_caller_identity(caller("source_a", "1", "11"));
        let context = AttemptContext::new(
            RequestAttemptContext::new(
                ModelRequestId::new("req_native_no_primary").unwrap(),
                ClientApiKeyId::new("key_shared").unwrap(),
            )
            .with_caller_identity(caller("source_a", "1", "11"))
            .with_request_location(Some(global_request_location())),
            NonZeroU32::MIN,
            SystemTime::now() + Duration::from_secs(5),
            account_policy(),
            AccountAttemptContext::new(BTreeSet::new(), None, Some(owner))
                .with_account_scope(contract_account_scope()),
            Some(ContinuationBinding::Pinned(pin)),
            CancellationToken::new(),
        );
        let stream = provider
            .execute(planned_request("openai", generate_operation()), context)
            .await
            .unwrap();
        assert_eq!(
            stream.metadata().provider_account_id().as_str(),
            "acct_subagent_b"
        );
        assert_eq!(affinity.binding_count(), usize::from(existing_primary));
        if existing_primary {
            assert_eq!(primary(&affinity), "acct_subagent_a");
        }
    }
}

#[tokio::test]
async fn token_management_reassign_clear_and_unavailable_target_keep_versioned_boundaries() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_subagent_a").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let provider = provider_with_affinity(&store, affinity.clone());
    let choose = || {
        provider.clone().execute(
            planned_request("openai", generate_operation()),
            token_context(contract_account_scope()),
        )
    };
    drop(choose().await.unwrap());
    create_account(&store, "acct_subagent_b").await;
    let (key, _) = affinity.binding_for_scope(ProviderAffinityScope::Token);
    affinity.set_directive(
        &key,
        SessionBindingDirective::Target {
            account_id: "acct_missing".into(),
        },
    );
    let pending = affinity.binding_for_scope(ProviderAffinityScope::Token).1;
    let stream = choose().await.unwrap();
    assert_eq!(
        stream.metadata().provider_account_id().as_str(),
        "acct_subagent_a"
    );
    drop(stream);
    assert_eq!(
        affinity
            .binding_for_scope(ProviderAffinityScope::Token)
            .1
            .revision(),
        pending.revision()
    );
    affinity.set_directive(&key, SessionBindingDirective::Reassign);
    assert_eq!(
        choose()
            .await
            .unwrap()
            .metadata()
            .provider_account_id()
            .as_str(),
        "acct_subagent_b"
    );
    assert_eq!(primary(&affinity), "acct_subagent_b");
    affinity.set_directive(&key, SessionBindingDirective::Clear);
    drop(choose().await.unwrap());
    assert!(
        affinity
            .binding_for_scope(ProviderAffinityScope::Token)
            .1
            .directive()
            .is_none()
    );
}

#[tokio::test]
async fn token_store_timeout_and_cancellation_do_not_start_upstream_or_claim_binding() {
    for cancel in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_subagent_a").await;
        let affinity = Arc::new(MemorySessionAffinity::with_operation_delay(
            Duration::from_secs(2),
        ));
        let server = MockServer::start().await;
        let provider = provider_with_affinity_and_base_url(&store, affinity.clone(), server.uri());
        let cancellation = CancellationToken::new();
        let context = scoped_context("req_timeout", "key_shared", caller("source_a", "1", "11"));
        let context = if cancel {
            cancellation.cancel();
            AttemptContext::new(
                RequestAttemptContext::new(
                    ModelRequestId::new("req_cancel").unwrap(),
                    ClientApiKeyId::new("key_shared").unwrap(),
                )
                .with_caller_identity(caller("source_a", "1", "11")),
                NonZeroU32::MIN,
                SystemTime::now() + Duration::from_secs(5),
                account_policy(),
                AccountAttemptContext::new(BTreeSet::new(), None, None)
                    .with_account_scope(contract_account_scope()),
                None,
                cancellation,
            )
        } else {
            context
        };
        assert!(
            timeout(
                Duration::from_secs(1),
                provider.execute(planned_request("openai", generate_operation()), context)
            )
            .await
            .unwrap()
            .is_err()
        );
        assert_eq!(affinity.binding_count(), 0);
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn quota_and_cooldown_spillover_keep_primary_and_recover_on_the_next_request() {
    use gateway_core::account::{QuotaEvidence, QuotaState};
    use gateway_core::provider_ports::ProviderCooldown;
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_subagent_a").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let cooldowns = Arc::new(MemoryCooldownPort::new());
    let (provider, _, _) = provider_and_quota_with_runtime_ports(
        &store,
        affinity.clone(),
        MockServer::start().await.uri(),
        Arc::new(TestLeaseCoordinator::default()),
        0,
        cooldowns.clone(),
        crate::support::runtime_policy(),
    );
    let choose = || {
        provider.clone().execute(
            planned_request("openai", generate_operation()),
            token_context(contract_account_scope()),
        )
    };
    drop(choose().await.unwrap());
    create_account(&store, "acct_subagent_b").await;
    let account = ProviderAccountId::new("acct_subagent_a").unwrap();
    for quota in [true, false] {
        if quota {
            store
                .apply_quota_access(QuotaAccessChange {
                    account_id: account.clone(),
                    expected_revision: store.account(account.as_str()).unwrap().revision(),
                    state: QuotaState::exhausted(
                        QuotaEvidence::UsageLimitReached,
                        SystemTime::now(),
                        None,
                    ),
                })
                .await
                .unwrap();
        } else {
            cooldowns.cooldowns.lock().unwrap().insert(
                account.clone(),
                ProviderCooldown::new(
                    account.clone(),
                    store.account(account.as_str()).unwrap().revision(),
                    SystemTime::now() + Duration::from_secs(60),
                ),
            );
        }
        let stream = choose().await.unwrap();
        assert_eq!(
            stream.metadata().provider_account_id().as_str(),
            "acct_subagent_b"
        );
        assert_eq!(primary(&affinity), "acct_subagent_a");
        drop(stream);
        store
            .apply_quota_access(QuotaAccessChange {
                account_id: account.clone(),
                expected_revision: store.account(account.as_str()).unwrap().revision(),
                state: QuotaState::allowed(SystemTime::now()),
            })
            .await
            .unwrap();
        cooldowns.cooldowns.lock().unwrap().clear();
        assert_eq!(
            choose()
                .await
                .unwrap()
                .metadata()
                .provider_account_id()
                .as_str(),
            "acct_subagent_a"
        );
    }
}

#[tokio::test]
async fn relaxed_token_requests_do_not_read_or_create_primary_bindings() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_subagent_a").await;
    let affinity = Arc::new(MemorySessionAffinity::with_operation_delay(
        Duration::from_secs(2),
    ));
    let provider = provider_with_affinity(&store, affinity.clone());
    let context = AttemptContext::new(
        RequestAttemptContext::new(
            ModelRequestId::new("req_relaxed").unwrap(),
            ClientApiKeyId::new("key_shared").unwrap(),
        )
        .with_caller_identity(caller("source_a", "1", "11"))
        .with_request_location(Some(global_request_location())),
        NonZeroU32::MIN,
        SystemTime::now() + Duration::from_secs(5),
        account_policy()
            .with_openai_account_affinity(gateway_core::account::AccountAffinity::Relaxed),
        AccountAttemptContext::new(BTreeSet::new(), None, None)
            .with_account_scope(contract_account_scope()),
        None,
        CancellationToken::new(),
    );
    drop(
        timeout(
            Duration::from_millis(400),
            provider.execute(planned_request("openai", generate_operation()), context),
        )
        .await
        .unwrap()
        .unwrap(),
    );
    assert_eq!(affinity.binding_count(), 0);
}

#[tokio::test]
async fn transport_retry_and_replay_any_never_claim_primary_or_consume_management() {
    for existing in [false, true] {
        for replay in [false, true] {
            let store = Arc::new(MemoryAccountStore::default());
            create_account(&store, "acct_subagent_a").await;
            let affinity = Arc::new(MemorySessionAffinity::default());
            let provider = provider_with_affinity(&store, affinity.clone());
            if existing {
                drop(
                    provider
                        .clone()
                        .execute(
                            planned_request("openai", generate_operation()),
                            token_context(contract_account_scope()),
                        )
                        .await
                        .unwrap(),
                );
                let (key, _) = affinity.binding_for_scope(ProviderAffinityScope::Token);
                affinity.set_directive(
                    &key,
                    SessionBindingDirective::Target {
                        account_id: "acct_subagent_a".into(),
                    },
                );
            }
            create_account(&store, "acct_subagent_b").await;
            let context = AttemptContext::new(
                RequestAttemptContext::new(
                    ModelRequestId::new("req_transport_retry").unwrap(),
                    ClientApiKeyId::new("key_shared").unwrap(),
                )
                .with_caller_identity(caller("source_a", "1", "11"))
                .with_request_location(Some(global_request_location())),
                NonZeroU32::new(if replay { 1 } else { 2 }).unwrap(),
                SystemTime::now() + Duration::from_secs(5),
                account_policy(),
                AccountAttemptContext::new(
                    BTreeSet::from([ProviderAccountId::new("acct_subagent_a").unwrap()]),
                    None,
                    None,
                )
                .with_account_scope(contract_account_scope()),
                None,
                CancellationToken::new(),
            )
            .with_continuation_attempt(if replay {
                ContinuationAttempt::ReplayAny
            } else {
                ContinuationAttempt::None
            });
            let stream = provider
                .clone()
                .execute(planned_request("openai", generate_operation()), context)
                .await
                .unwrap();
            assert_eq!(
                stream.metadata().provider_account_id().as_str(),
                "acct_subagent_b"
            );
            assert_eq!(affinity.binding_count(), usize::from(existing));
            if existing {
                let binding = affinity.binding_for_scope(ProviderAffinityScope::Token).1;
                assert_eq!(binding.account_id().as_str(), "acct_subagent_a");
                assert!(binding.directive().is_some());
            }
        }
    }
}

#[tokio::test]
async fn pending_target_unavailable_for_model_or_capacity_keeps_chat_and_retries_later() {
    use gateway_core::account::{AccountModelAccess, AccountModelAccessMode};
    for busy in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_subagent_a").await;
        let affinity = Arc::new(MemorySessionAffinity::default());
        let leases = Arc::new(TestLeaseCoordinator::default());
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream"),
            )
            .expect(3)
            .mount(&server)
            .await;
        let provider = provider_with_affinity_and_base_url_and_leases(
            &store,
            affinity.clone(),
            server.uri(),
            leases.clone(),
        );
        let run = |scope| {
            provider.clone().execute(
                planned_request("openai", http_generate_operation()),
                token_context(scope),
            )
        };
        let mut stream = run(contract_account_scope()).await.unwrap();
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
        create_account(&store, "acct_subagent_b").await;
        let (key, _) = affinity.binding_for_scope(ProviderAffinityScope::Token);
        affinity.set_directive(
            &key,
            SessionBindingDirective::Target {
                account_id: "acct_subagent_b".into(),
            },
        );
        let pending = affinity.binding_for_scope(ProviderAffinityScope::Token).1;
        let scope = if busy {
            leases
                .busy_accounts
                .lock()
                .unwrap()
                .insert(ProviderAccountId::new("acct_subagent_b").unwrap());
            contract_account_scope()
        } else {
            Arc::new(FrozenAccountScope::new(
                Arc::new(RuntimeAccountDirectory::new(
                    [
                        ("acct_subagent_a", "gpt-5.4"),
                        ("acct_subagent_b", "gpt-other"),
                    ]
                    .into_iter()
                    .map(|(id, model)| {
                        (
                            ProviderAccountId::new(id).unwrap(),
                            RuntimeAccount::new(
                                ProviderKind::new("openai").unwrap(),
                                BTreeSet::new(),
                            )
                            .with_model_access(
                                AccountModelAccess::new(
                                    AccountModelAccessMode::Allowlist,
                                    vec![model.into()],
                                )
                                .unwrap(),
                            ),
                        )
                    })
                    .collect(),
                )),
                ClientRoutingScope::all_accounts(),
            ))
        };
        let mut stream = run(scope).await.unwrap();
        assert_eq!(
            stream.metadata().provider_account_id().as_str(),
            "acct_subagent_a"
        );
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
        assert_eq!(
            affinity
                .binding_for_scope(ProviderAffinityScope::Token)
                .1
                .revision(),
            pending.revision()
        );
        leases.busy_accounts.lock().unwrap().clear();
        let mut stream = run(contract_account_scope()).await.unwrap();
        assert_eq!(
            stream.metadata().provider_account_id().as_str(),
            "acct_subagent_b"
        );
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
        assert_eq!(primary(&affinity), "acct_subagent_b");
        assert!(
            affinity
                .binding_for_scope(ProviderAffinityScope::Token)
                .1
                .directive()
                .is_none()
        );
        server.verify().await;
    }
}

#[tokio::test]
async fn reassign_without_an_alternative_keeps_serving_and_preserves_pending_change() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_subagent_a").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let provider = provider_with_affinity(&store, affinity.clone());
    let choose = || {
        provider.clone().execute(
            planned_request("openai", generate_operation()),
            token_context(contract_account_scope()),
        )
    };
    drop(choose().await.unwrap());
    let (key, _) = affinity.binding_for_scope(ProviderAffinityScope::Token);
    affinity.set_directive(&key, SessionBindingDirective::Reassign);
    let pending = affinity.binding_for_scope(ProviderAffinityScope::Token).1;
    let stream = choose().await.unwrap();
    assert_eq!(
        stream.metadata().provider_account_id().as_str(),
        "acct_subagent_a"
    );
    drop(stream);
    assert_eq!(
        affinity
            .binding_for_scope(ProviderAffinityScope::Token)
            .1
            .revision(),
        pending.revision()
    );
}
