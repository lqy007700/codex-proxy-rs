//! 验证 OAuth 换号后的真实身份续接隔离和旧状态兼容边界

use provider_openai::credential::{CodexCredentialAdmin, CodexCredentialCodec};
use sha2::{Digest, Sha256};

use super::*;

#[tokio::test]
async fn oauth_continuations_require_matching_identity_after_slot_reauthorization() {
    for (changed, identity, rejected) in [
        (false, "legacy", false),
        (true, "legacy", true),
        (false, "different", true),
        (true, "current", false),
    ] {
        let store = Arc::new(MemoryAccountStore::default());
        let id = "acct_provider_contract";
        let mut account = CodexCredentialAdmin
            .prepare_import(ImportCodexOAuthCredential {
                account_id: id.into(),
                name: "local account".into(),
                secret: secret("access-test-only"),
                verified_account: profile(id),
                next_refresh_at: None,
                enabled: true,
            })
            .unwrap();
        let mut hash = Sha256::new();
        for part in [
            id,
            account.account.upstream_user_id().unwrap(),
            account.account.upstream_account_id().unwrap(),
        ] {
            hash.update((part.len() as u64).to_le_bytes());
            hash.update(part.as_bytes());
        }
        let current_identity = hex::encode(hash.finalize());
        let mut data = CodexCredentialCodec::decode_complete(&account.credential).unwrap();
        data.oauth_mut().unwrap().oauth_identity_changed = changed;
        account.credential = CodexCredentialCodec::encode_complete(data).unwrap();
        store.create_account(account).await.unwrap();
        let upstream = MockServer::start().await;
        let provider = provider_with_base_url(&store, upstream.uri());
        let mut state = json!({"account_id":id, "continuation_scope":"persisted"});
        if identity != "legacy" {
            state["oauth_identity"] = json!(if identity == "current" {
                &current_identity
            } else {
                "old-identity"
            });
        }
        let payload = ProtocolPayload::json_object("openai", json!({
            "model":"gpt-5.4", "input":[{"type":"function_call_output", "call_id":"call_previous", "output":"result"}],
            "previous_response_id":"client_previous", "store":true
        }).as_object().unwrap().clone()).unwrap();
        let generate = GenerateRequest::from_protocol_payload(payload).with_provider_session_state(
            ProviderSessionState::new("openai", state.as_object().unwrap().clone()).unwrap(),
        );
        let result = provider
            .execute(
                planned_request("openai", Operation::Generate(generate)),
                pinned_continuation_context(
                    "req_identity_boundary",
                    id,
                    "client_previous",
                    "upstream_previous",
                    1,
                    ContinuationAttempt::Native,
                ),
            )
            .await;
        if rejected {
            let error = result
                .err()
                .expect("changed identity requires client replay");
            assert_eq!(
                error.kind(),
                ProviderErrorKind::ContinuationRecoveryRequired
            );
            assert_eq!(error.send_state(), UpstreamSendState::NotSent);
            assert_eq!(
                error.continuation_recovery_disposition(),
                Some(ContinuationRecoveryDisposition::ClientReplayRequired)
            );
        } else {
            assert!(result.is_ok(), "同身份及未换号的旧记录应保持续接能力");
        }
        assert!(upstream.received_requests().await.unwrap().is_empty());
    }
}
