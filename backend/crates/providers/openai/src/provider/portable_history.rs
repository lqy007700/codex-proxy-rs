//! Key 显式开启的跨渠道历史转换与不可迁移状态边界

use super::*;

pub(super) fn enabled(context: &AttemptContext) -> bool {
    context
        .account_scope()
        .is_some_and(|scope| scope.portable_history())
}

/// 在选号前拒绝无法从本次输入恢复的历史，不把增量请求当作完整上下文
pub(super) fn validate(
    request: &CodexResponsesRequest,
    context: &AttemptContext,
) -> Result<(), ProviderError> {
    if !enabled(context) {
        return Ok(());
    }
    if request.previous_response_id().is_some() || context.continuation().is_some() {
        return Err(continuation_replay_required_error(
            "portable_history_requires_full_input",
        ));
    }
    if request
        .body()
        .get("conversation")
        .is_some_and(|value| !value.is_null())
    {
        return Err(unavailable("conversation_reference"));
    }
    let Some(input) = request.body().get("input").and_then(Value::as_array) else {
        return Ok(());
    };
    for item in input {
        let item_type = item.get("type").and_then(Value::as_str);
        if item_type == Some("reasoning") {
            continue;
        }
        if matches!(
            item_type,
            Some("compaction" | "compaction_summary" | "context_compaction" | "item_reference")
        ) {
            return Err(unavailable("opaque_history"));
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
            return Err(unavailable("encrypted_business_content"));
        }
    }
    // reasoning 不能替代可读历史；没有任何其余输入时显式失败，避免制造失忆的新请求
    if !input.is_empty() && input.iter().all(is_reasoning) {
        return Err(unavailable("readable_history_missing"));
    }
    Ok(())
}

pub(super) fn apply(
    request: &mut CodexResponsesRequest,
    context: &AttemptContext,
) -> Result<(), ProviderError> {
    validate(request, context)?;
    if !enabled(context) {
        return Ok(());
    }
    let removed = if let Some(input) = request
        .body_mut()
        .get_mut("input")
        .and_then(Value::as_array_mut)
    {
        let before = input.len();
        input.retain(|item| !is_reasoning(item));
        before - input.len()
    } else {
        0
    };
    // 兼容模式始终创建新链，保留逻辑会话亲和但不继承上一个渠道的轮次状态
    clear_request_turn_state(request);
    context.trace().record_evidence(
        "provider.request.portable_history",
        json!({
            "enabled": true,
            "removedReasoningItems": removed,
        }),
    );
    Ok(())
}

fn is_reasoning(item: &Value) -> bool {
    item.get("type").and_then(Value::as_str) == Some("reasoning")
}

fn unavailable(reason: &'static str) -> ProviderError {
    provider_error(ProviderErrorKind::InvalidRequest, UpstreamSendState::NotSent)
        .with_retry_prohibited()
        .with_diagnostic(ProviderDiagnostic::new("portable history requires readable messages and tool results")
            .with_classification("prepare", reason))
        .with_client_visible_upstream_error(ClientVisibleUpstreamError::new(
            "Cross-channel history contains state that cannot be migrated. Replay readable messages and tool results, or start a new conversation with a readable summary.",
            Some("portable_history_unavailable".to_owned()),
            Some("invalid_request_error".to_owned()),
        ))
}
