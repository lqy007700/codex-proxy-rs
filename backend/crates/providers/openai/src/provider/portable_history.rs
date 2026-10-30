//! 明确密文拒绝后的同账号请求恢复与精确旧推理排除，不保存聊天正文

use std::collections::BTreeSet;

use gateway_core::provider_ports::{
    MAX_HISTORY_EXCLUSIONS, ProviderHistoryExclusionPort, ProviderSessionAffinityKey,
};
use gateway_core::upstream::RequestRecoveryData;
use sha2::{Digest, Sha256};

use super::*;

const CACHE_TIMEOUT: Duration = Duration::from_millis(100);

#[derive(Clone)]
struct HistoryRecovery {
    scope: ProviderSessionAffinityKey,
    account_id: String,
    fingerprints: BTreeSet<String>,
}

pub(super) struct HistoryAttempt {
    recovery: HistoryRecovery,
    candidates: BTreeSet<String>,
    eligibility: Result<(), &'static str>,
    incremental: bool,
    pending_commit: bool,
    ttl: Duration,
}

fn enabled(context: &AttemptContext) -> bool {
    !context.is_diagnostic_required_account()
        && context
            .account_scope()
            .is_some_and(|scope| scope.portable_history())
}

struct DigestWriter(Sha256);

impl std::io::Write for DigestWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn digest_json(domain: &[u8], value: &impl Serialize) -> Option<String> {
    let mut writer = DigestWriter(Sha256::new());
    writer.0.update(domain);
    serde_json::to_writer(&mut writer, value).ok()?;
    Some(hex::encode(writer.0.finalize()))
}

fn fingerprint(item: &Value) -> Option<String> {
    if item.get("type").and_then(Value::as_str) != Some("reasoning") {
        return None;
    }
    let encrypted = item
        .get("encrypted_content")?
        .as_str()
        .filter(|value| !value.trim().is_empty())?;
    // 流式计算固定身份序列，不复制密文或整份历史
    digest_json(
        b"codex-history-item-v1\0",
        &(item.get("id").and_then(Value::as_str), encrypted),
    )
}

fn candidates(request: &CodexResponsesRequest) -> BTreeSet<String> {
    request
        .body()
        .get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(fingerprint)
        .collect()
}

fn filter(request: &mut CodexResponsesRequest, excluded: &BTreeSet<String>) -> usize {
    let Some(input) = request
        .body_mut()
        .get_mut("input")
        .and_then(Value::as_array_mut)
    else {
        return 0;
    };
    let before = input.len();
    input.retain(|item| {
        fingerprint(item).is_none_or(|fingerprint| !excluded.contains(&fingerprint))
    });
    before - input.len()
}

/// 只能校验请求自包含，无法证明客户端没有省略早先消息
fn replay_eligibility(
    request: &CodexResponsesRequest,
    context: &AttemptContext,
) -> Result<(), &'static str> {
    if request.previous_response_id().is_some() || context.continuation().is_some() {
        return Err("incremental_history");
    }
    if request
        .body()
        .get("conversation")
        .is_some_and(|value| !value.is_null())
    {
        return Err("conversation_reference");
    }
    let input = request
        .body()
        .get("input")
        .and_then(Value::as_array)
        .ok_or("readable_history_missing")?;
    let mut calls = BTreeSet::new();
    let mut message_seen = false;
    for item in input {
        let kind = item.get("type").and_then(Value::as_str);
        if kind == Some("reasoning") {
            continue;
        }
        if ["encrypted_content", "encrypted_function_args"]
            .iter()
            .any(|key| item.get(key).is_some_and(|value| !value.is_null()))
            || ["content", "output"].iter().any(|key| {
                item.get(key)
                    .and_then(Value::as_array)
                    .is_some_and(|parts| {
                        parts.iter().any(|part| {
                            part.get("type").and_then(Value::as_str) == Some("encrypted_content")
                        })
                    })
            })
        {
            return Err("encrypted_business_content");
        }
        match kind {
            Some("message") | None if item.get("role").and_then(Value::as_str).is_some() => {
                message_seen |= item.get("content").is_some_and(|content| match content {
                    Value::String(text) => !text.trim().is_empty(),
                    Value::Array(parts) => !parts.is_empty(),
                    _ => false,
                });
            }
            Some("function_call" | "custom_tool_call" | "local_shell_call" | "computer_call") => {
                let call_id = item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .ok_or("tool_call_id_missing")?;
                if !calls.insert(call_id) {
                    return Err("tool_call_id_duplicate");
                }
            }
            Some(
                "function_call_output"
                | "custom_tool_call_output"
                | "local_shell_call_output"
                | "computer_call_output",
            ) => {
                if !item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| calls.remove(id))
                {
                    return Err("tool_call_missing");
                }
            }
            // 未知项也可能依赖服务器状态，恢复时不猜测其可迁移性
            _ => return Err("opaque_history"),
        }
    }
    if !calls.is_empty() {
        return Err("tool_output_missing");
    }
    if !message_seen {
        return Err("readable_history_missing");
    }
    Ok(())
}

impl HistoryAttempt {
    pub(super) async fn prepare(
        request: &mut CodexResponsesRequest,
        context: &AttemptContext,
        lease: &CodexCredentialLease,
        response_origin: &Url,
        cache: &dyn ProviderHistoryExclusionPort,
        thread: Option<&str>,
    ) -> Result<Option<Self>, ProviderError> {
        if !enabled(context) {
            return Ok(None);
        }
        let initial_candidates = candidates(request);
        // 正常无密文输入不访问 Redis，也不复制整份请求
        if initial_candidates.is_empty()
            && context.request_recovery().is_none()
            && request.previous_response_id().is_none()
            && context.continuation().is_none()
        {
            return Ok(None);
        }
        let endpoint = lease
            .authentication()
            .responses_api_base_url()
            .unwrap_or(response_origin.as_str());
        let scope_fields = (
            "codex-history-scope-v1",
            context.client_api_key_ref().as_str(),
            PROVIDER_NAME,
            thread.or(request.client_account_session_id.as_deref()),
            lease.account_id().as_str(),
            lease.account().revision().get(),
            endpoint,
        );
        let scope = ProviderSessionAffinityKey::try_new(
            digest_json(b"codex-history-scope-v1\0", &scope_fields)
                .ok_or_else(|| unavailable("history_scope"))?,
        )
        .map_err(|_| unavailable("history_scope"))?;
        let eligibility = replay_eligibility(request, context);
        let ttl = context
            .account_selection_policy()
            .openai_session_affinity_ttl()
            .min(Duration::from_secs(24 * 60 * 60));
        let approved = context
            .request_recovery()
            .and_then(RequestRecoveryData::downcast_ref::<HistoryRecovery>);
        let mut excluded = BTreeSet::new();
        let mut pending_commit = false;
        if let Some(approved) =
            approved.filter(|approved| approved.account_id == lease.account_id().as_str())
        {
            if approved.scope != scope {
                return Err(unavailable("history_target_changed"));
            }
            eligibility.map_err(unavailable)?;
            excluded.clone_from(&approved.fingerprints);
            pending_commit = true;
        } else if eligibility.is_ok() && initial_candidates.len() <= MAX_HISTORY_EXCLUSIONS {
            let result = tokio::select! {
                biased;
                () = context.cancellation().cancelled() => return Err(provider_error(ProviderErrorKind::Cancelled, UpstreamSendState::NotSent)),
                result = tokio::time::timeout(CACHE_TIMEOUT, cache.matching(&scope, &initial_candidates, ttl)) => result,
            };
            match result {
                Ok(Ok(matching)) => excluded = matching,
                _ => context.trace().record_evidence(
                    "provider.history_cache",
                    json!({"operation":"matching", "available":false}),
                ),
            }
        }
        let removed = if excluded.is_empty() {
            0
        } else {
            filter(request, &excluded)
        };
        if pending_commit {
            // 后续 attempt 仍从原始输入编码，必须同时重用已批准的排除集和新链边界
            clear_request_turn_state(request);
        }
        if removed > 0 {
            context.trace().record_evidence(
                "provider.request.portable_history",
                json!({"removedReasoningItems":removed, "requestRecovery":pending_commit}),
            );
        }
        let candidates = initial_candidates.difference(&excluded).cloned().collect();
        Ok(Some(Self {
            recovery: HistoryRecovery {
                scope,
                account_id: lease.account_id().as_str().to_owned(),
                fingerprints: excluded,
            },
            candidates,
            eligibility,
            incremental: request.previous_response_id().is_some()
                || context.continuation().is_some(),
            pending_commit,
            ttl,
        }))
    }

    fn map_error(
        &self,
        error: ProviderError,
        replay_closed: bool,
        context: &AttemptContext,
    ) -> ProviderError {
        let explicit_rejection = error.upstream_failure_fields().is_some_and(|fields| {
            fields.code.as_deref().is_some_and(|code| {
                code.trim()
                    .eq_ignore_ascii_case("invalid_encrypted_content")
            }) && matches!(fields.status, None | Some(200 | 400 | 409 | 422))
        });
        if !explicit_rejection
            || replay_closed
            || error.send_state() == UpstreamSendState::Ambiguous
            || error.retry_is_prohibited()
            || context.cancellation().is_cancelled()
            || context.deadline().is_elapsed()
        {
            return error;
        }
        if self.incremental {
            return project_continuation_replay_error(
                error,
                "encrypted_history_requires_full_input",
            );
        }
        if let Err(reason) = self.eligibility {
            context.trace().record_evidence(
                "provider.history_recovery",
                json!({"eligible":false,"reason":reason}),
            );
            return error;
        }
        if self.candidates.is_empty() || context.request_recovery().is_some() {
            return error;
        }
        let mut recovery = self.recovery.clone();
        recovery
            .fingerprints
            .extend(self.candidates.iter().cloned());
        error.with_request_recovery(RequestRecoveryData::new(recovery))
    }

    pub(super) fn wrap(
        self,
        mut events: EventStream,
        context: AttemptContext,
        cache: Arc<dyn ProviderHistoryExclusionPort>,
    ) -> EventStream {
        Box::pin(async_stream::try_stream! {
            let mut state = self;
            let mut replay_closed = false;
            while let Some(event) = events.next().await {
                let event = event.map_err(|error| state.map_error(error, replay_closed, &context))?;
                let semantic_output = event.wire_event().is_some_and(|wire| {
                    crate::transport::protocol::responses::response_event_signals(wire.event_type(), wire.data()).semantic_output
                }) || event.canonical_facts().iter().any(|fact| matches!(fact,
                    GatewayEvent::TextDelta(_) | GatewayEvent::ReasoningDelta(_) | GatewayEvent::ToolCallDelta(_)));
                let completed = event.canonical_facts().iter().any(|fact| matches!(fact, GatewayEvent::Completed(_)))
                    || event.wire_event().is_some_and(|wire| wire.event_type() == Some("response.completed")
                        && wire.data().pointer("/response/id").and_then(Value::as_str).is_some_and(|id| !id.is_empty())
                        && wire.data().pointer("/response/status").and_then(Value::as_str) == Some("completed"));
                replay_closed |= event.has_client_event() && !event.is_replayable_prelude();
                if state.pending_commit && (semantic_output || completed) {
                    state.pending_commit = false;
                    let result = tokio::select! {
                        biased;
                        () = context.cancellation().cancelled() => None,
                        result = tokio::time::timeout(CACHE_TIMEOUT, cache.merge(&state.recovery.scope, &state.recovery.fingerprints, state.ttl)) => Some(result),
                    };
                    let stored = matches!(result, Some(Ok(Ok(true))));
                    context.trace().record_evidence("provider.history_cache", json!({"operation":"merge", "stored":stored, "excludedItems":state.recovery.fingerprints.len()}));
                }
                yield event;
            }
        })
    }
}

fn unavailable(reason: &'static str) -> ProviderError {
    provider_error(ProviderErrorKind::InvalidRequest, UpstreamSendState::NotSent)
        .with_retry_prohibited()
        .with_diagnostic(ProviderDiagnostic::new("history recovery cannot replay this input").with_classification("prepare", reason))
        .with_client_visible_upstream_error(ClientVisibleUpstreamError::new(
            "History recovery requires self-contained readable messages and paired tool calls and results.",
            Some("portable_history_unavailable".to_owned()), Some("invalid_request_error".to_owned()),
        ))
}
