//! 验证聚合器的未知字段、累计用量与截断检查

use gateway_protocol::anthropic::MessagesAccumulator;
use serde_json::json;

#[test]
fn accumulator_preserves_native_fields_and_merges_usage() {
    let mut stream = MessagesAccumulator::default();
    for event in [
        json!({"type":"message_start","message":{"id":"m1","type":"message","content":[],"future_field":{"enabled":true},"usage":{"input_tokens":4,"cache_read_input_tokens":5}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"c1","name":"lookup","input":{},"future_tool_field":7}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"q\":"}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"hi\"}"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":9}}),
    ] {
        assert!(stream.push(&event).expect("聚合事件").is_none());
    }
    let message = stream
        .push(&json!({"type":"message_stop"}))
        .expect("正常结束")
        .expect("完整消息");
    stream.finish().expect("终态完整");
    assert_eq!(message["future_field"]["enabled"], true);
    assert_eq!(message["content"][0]["future_tool_field"], 7);
    assert_eq!(message["content"][0]["input"], json!({"q":"hi"}));
    assert_eq!(
        message["usage"],
        json!({"input_tokens":4,"cache_read_input_tokens":5,"output_tokens":9})
    );
}

#[test]
fn explicit_errors_finish_but_transport_truncation_does_not() {
    let mut stream = MessagesAccumulator::default();
    assert!(stream.finish().is_err());
    let error = json!({"type":"error","error":{"type":"api_error","message":"Interrupted."}});
    assert_eq!(stream.push(&error).expect("错误终态"), Some(error));
    assert!(stream.finish().is_ok());
    assert!(stream.push(&json!({"type":"message_stop"})).is_err());
}

#[test]
fn native_json_rejects_unparseable_tool_input_at_message_stop() {
    let mut stream = MessagesAccumulator::default();
    for event in [
        json!({"type":"message_start","message":{"content":[]}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","input":{}}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","delta":{"stop_reason":"max_tokens"}}),
    ] {
        stream.push(&event).expect("输入分片");
    }
    assert!(stream.finish().is_err());
    assert!(stream.push(&json!({"type":"message_stop"})).is_err());
}

#[test]
fn official_sdk_tool_fixture_survives_arbitrary_byte_boundaries() {
    // 依据官方 SDK v0.75.0 的 test_tool_use 与 fixtures/tool_use_response.txt
    // https://github.com/anthropics/anthropic-sdk-python/blob/v0.75.0/tests/lib/streaming/test_messages.py
    let fixture = include_bytes!("fixtures/tool_use_response.sse");
    for chunk_size in [1, 2, 7, 64, fixture.len()] {
        let mut decoder = gateway_protocol::openai::sse::SseEventDecoder::default();
        let mut stream = MessagesAccumulator::default();
        let mut message = None;
        for chunk in fixture.chunks(chunk_size) {
            for event in decoder.push(chunk).expect("解析官方 SSE 字节流") {
                let value = serde_json::from_str(&event.data).expect("事件 JSON");
                if let Some(complete) = stream.push(&value).expect("聚合官方 SSE") {
                    message = Some(complete);
                }
            }
        }
        stream.finish().expect("官方 fixture 有完整终态");
        let message = message.expect("工具响应");
        assert_eq!(message["id"], "msg_019Q1hrJbZG26Fb9BQhrkHEr");
        assert_eq!(message["stop_reason"], "tool_use");
        assert_eq!(
            message["content"][0]["text"],
            "I'll check the current weather in Paris for you."
        );
        assert_eq!(
            message["content"][1]["id"],
            "toolu_01NRLabsLyVHZPKxbKvkfSMn"
        );
        assert_eq!(message["content"][1]["name"], "get_weather");
        assert_eq!(message["content"][1]["input"], json!({"location":"Paris"}));
        assert_eq!(message["usage"]["input_tokens"], 377);
        assert_eq!(message["usage"]["output_tokens"], 65);
        assert_eq!(message["usage"]["service_tier"], "standard");
    }
}

#[test]
fn native_json_to_sse_round_trip_preserves_all_payload_fields() {
    let source = json!({
        "id":"msg_native", "type":"message", "role":"assistant", "model":"alias",
        "future_field":{"enabled":true}, "stop_reason":"stop_sequence", "stop_sequence":"停止",
        "content":[
            {"type":"text","text":"答复","citations":null,"future_text":7},
            {"type":"thinking","thinking":"推理","signature":"native","future_thinking":9},
            {"type":"future_block","opaque":{"key":[1,2,3]}}
        ],
        "usage":{"input_tokens":4,"output_tokens":9,"cache_creation":{"ephemeral_1h_input_tokens":3},"server_tool_use":{"web_search_requests":1}}
    });
    let mut stream = MessagesAccumulator::default();
    let mut complete = None;
    for event in gateway_protocol::anthropic::message_to_stream_events(source.clone()).unwrap() {
        if let Some(message) = stream.push(&event).unwrap() {
            complete = Some(message);
        }
    }
    assert_eq!(complete, Some(source));
    stream.finish().unwrap();
}

#[test]
fn citations_signatures_and_optional_usage_follow_official_sdk_semantics() {
    // v2.8.0 accumulate_event：citations 初值可为 null，签名覆盖，usage 为累计量
    let mut stream = MessagesAccumulator::default();
    let citation = json!({"type":"char_location","cited_text":"来源","document_index":0,"document_title":null,"start_char_index":0,"end_char_index":2});
    for event in [
        json!({"type":"message_start","message":{"type":"message","content":[],"usage":{"input_tokens":4,"output_tokens":1,"cache_read_input_tokens":5,"server_tool_use":{"web_search_requests":1}}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"","citations":null}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"正文"}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"citations_delta","citation":citation}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"thinking","thinking":"","signature":""}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"thinking_delta","thinking":"思考"}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"signature_delta","signature":"first"}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"signature_delta","signature":"final"}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null,"future_delta":7},"usage":{"output_tokens":3,"cache_read_input_tokens":null,"server_tool_use":null}}),
        json!({"type":"message_delta","delta":{},"usage":{"output_tokens":9}}),
    ] {
        stream.push(&event).unwrap();
    }
    let message = stream
        .push(&json!({"type":"message_stop"}))
        .unwrap()
        .unwrap();
    assert_eq!(message["content"][0]["citations"], json!([citation]));
    assert_eq!(message["content"][1]["thinking"], "思考");
    assert_eq!(message["content"][1]["signature"], "final");
    assert_eq!(message["future_delta"], 7);
    assert_eq!(
        message["usage"],
        json!({"input_tokens":4,"output_tokens":9,"cache_read_input_tokens":5,"server_tool_use":{"web_search_requests":1}})
    );
}

#[test]
fn invalid_block_lifecycles_do_not_produce_success() {
    let mut before_start = MessagesAccumulator::default();
    assert!(before_start.push(&json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})).is_err());
    let mut open = MessagesAccumulator::default();
    open.push(&json!({"type":"message_start","message":{"content":[]}}))
        .unwrap();
    open.push(
        &json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
    )
    .unwrap();
    assert!(open.push(&json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})).is_err());
    assert!(open.push(&json!({"type":"message_stop"})).is_err());
    assert!(open.finish().is_err());
    open.push(&json!({"type":"content_block_stop","index":0}))
        .unwrap();
    assert!(open.push(&json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"late"}})).is_err());
}

#[test]
fn omitted_container_updates_do_not_erase_native_identity() {
    let mut stream = MessagesAccumulator::default();
    let container = json!({"id":"container_native","expires_at":"2026-11-12T00:00:00Z"});
    stream
        .push(&json!({"type":"message_start","message":{"content":[],"container":container}}))
        .unwrap();
    stream.push(&json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null,"container":null}})).unwrap();
    let message = stream
        .push(&json!({"type":"message_stop"}))
        .unwrap()
        .unwrap();
    assert_eq!(message["container"], container);
    assert!(message["stop_sequence"].is_null());
}
