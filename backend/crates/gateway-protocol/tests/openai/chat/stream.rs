//! Chat 流的工具索引、终态、拒答与重复输出防护测试

use gateway_protocol::openai::chat::ChatStreamEncoder;
use serde_json::json;

use super::response::response;

fn encoder(include_usage: bool) -> ChatStreamEncoder {
    let mut encoder = ChatStreamEncoder::new("fallback".into(), 0, include_usage);
    let chunks = encoder
        .push(
            "response.created",
            &json!({"response":{"id":"resp_synthetic","model":"model-a","created_at":123}}),
        )
        .unwrap();
    assert_eq!(chunks[0]["choices"][0]["delta"]["role"], "assistant");
    encoder
}

#[test]
fn interleaved_tools_keep_stable_indices_and_do_not_repeat_arguments() {
    let mut encoder = encoder(true);
    let a =
        json!({"id":"fc_a","type":"function_call","call_id":"call_a","name":"a","arguments":"{}"});
    let b = json!({"id":"fc_b","type":"function_call","call_id":"call_b","name":"b","arguments":"{\"x\":1}"});
    for (index, item) in [&a, &b].into_iter().enumerate() {
        let chunks = encoder
            .push(
                "response.output_item.added",
                &json!({"item":item,"output_index":index+1}),
            )
            .unwrap();
        assert_eq!(
            chunks[0]["choices"][0]["delta"]["tool_calls"][0]["index"],
            index
        );
        assert_eq!(
            chunks[0]["choices"][0]["delta"]["tool_calls"][0]["id"],
            item["call_id"]
        );
    }
    for (id, index, part) in [("fc_b", 1, "{\"x\":"), ("fc_a", 0, "{}"), ("fc_b", 1, "1}")] {
        let chunks = encoder
            .push(
                "response.function_call_arguments.delta",
                &json!({"item_id":id,"delta":part}),
            )
            .unwrap();
        assert_eq!(
            chunks[0]["choices"][0]["delta"]["tool_calls"][0],
            json!({"index":index,"function":{"arguments":part}})
        );
    }
    assert!(
        encoder
            .push(
                "response.function_call_arguments.done",
                &json!({"item_id":"fc_b","arguments":"{\"x\":1}"})
            )
            .unwrap()
            .is_empty()
    );
    let chunks = encoder
        .push(
            "response.completed",
            &json!({"response":response(json!([{"type":"reasoning"},a,b]))}),
        )
        .unwrap();
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0]["choices"][0]["finish_reason"], "tool_calls");
    assert_eq!(chunks[1]["choices"], json!([]));
    assert_eq!(chunks[1]["usage"]["total_tokens"], 17);
}

#[test]
fn done_only_items_restore_text_and_arguments_once() {
    let mut encoder = encoder(false);
    let item = json!({"type":"message","content":[{"type":"output_text","text":"hello"}]});
    let chunks = encoder
        .push(
            "response.output_item.done",
            &json!({"output_index":0,"item":item}),
        )
        .unwrap();
    assert_eq!(chunks[0]["choices"][0]["delta"]["content"], "hello");
    let chunks = encoder
        .push(
            "response.completed",
            &json!({"response":response(json!([item]))}),
        )
        .unwrap();
    assert_eq!(chunks.len(), 1);
    assert!(chunks[0].get("usage").is_none());
}

#[test]
fn text_deltas_are_not_duplicated_by_done_or_terminal_output() {
    let mut encoder = encoder(false);
    encoder
        .push(
            "response.output_text.delta",
            &json!({"output_index":0,"content_index":0,"delta":"hello"}),
        )
        .unwrap();
    assert!(
        encoder
            .push(
                "response.output_text.done",
                &json!({"output_index":0,"content_index":0,"text":"hello"})
            )
            .unwrap()
            .is_empty()
    );
    let chunks=encoder.push("response.completed",&json!({"response":response(json!([{"type":"message","content":[{"type":"output_text","text":"hello"}]}]))})).unwrap();
    assert_eq!(chunks.len(), 1);
}

#[test]
fn failures_have_error_payload_without_successful_finish_reason() {
    let mut encoder = encoder(false);
    let chunks=encoder.push("response.failed",&json!({"response":{"id":"resp_synthetic","error":{"type":"server_error","code":"server_error","message":"failed"}}})).unwrap();
    assert_eq!(
        chunks,
        vec![json!({"error":{"type":"server_error","code":"server_error","message":"failed"}})]
    );
    assert!(encoder.is_terminal());
    assert!(
        encoder
            .push(
                "response.completed",
                &json!({"response":response(json!([]))})
            )
            .is_err()
    );
}

#[test]
fn rejects_changed_identity_and_unknown_tool_arguments() {
    let mut encoder = encoder(false);
    assert!(
        encoder
            .push("response.created", &json!({"response":{"id":"another"}}))
            .is_err()
    );
    assert!(
        encoder
            .push(
                "response.function_call_arguments.delta",
                &json!({"item_id":"missing","delta":"{}"})
            )
            .is_err()
    );
}

#[test]
fn refuses_unsupported_output_instead_of_claiming_an_empty_success() {
    let mut encoder = encoder(false);
    assert!(
        encoder
            .push(
                "response.completed",
                &json!({"response":response(json!([{"type":"image_generation_call"}]))})
            )
            .is_err()
    );
}
