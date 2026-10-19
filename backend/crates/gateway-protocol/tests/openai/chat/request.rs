//! Chat 历史、工具 schema、图片和不支持参数的转换测试

use gateway_protocol::openai::chat::decode_chat_request;
use serde_json::{Value, json};

fn convert(value: Value) -> Value {
    Value::Object(
        decode_chat_request(value.to_string().as_bytes())
            .unwrap()
            .responses,
    )
}

#[test]
fn preserves_ordered_instruction_messages_and_image_parts() {
    let result = convert(json!({"model":"model-a","messages":[
        {"role":"system","content":"first"},
        {"role":"developer","content":"second"},
        {"role":"user","content":[{"type":"text","text":"describe"},{"type":"image_url","image_url":{"url":"data:image/png;base64,synthetic","detail":"high"}}]},
        {"role":"assistant","content":"previous"}
    ]}));
    assert_eq!(
        result["input"],
        json!([
            {"type":"message","role":"system","content":[{"type":"input_text","text":"first"}]},
            {"type":"message","role":"developer","content":[{"type":"input_text","text":"second"}]},
            {"type":"message","role":"user","content":[{"type":"input_text","text":"describe"},{"type":"input_image","image_url":"data:image/png;base64,synthetic","detail":"high"}]},
            {"type":"message","role":"assistant","content":[{"type":"output_text","text":"previous"}]}
        ])
    );
    assert_eq!(result["store"], false);
}

#[test]
fn translates_parallel_function_history_using_call_id() {
    let result = convert(json!({"model":"model-a","messages":[
        {"role":"assistant","content":null,"tool_calls":[
            {"id":"call_a","type":"function","function":{"name":"weather","arguments":"{\"city\":\"a\"}"}},
            {"id":"call_b","type":"function","function":{"name":"weather","arguments":"{\"city\":\"b\"}"}}
        ]},
        {"role":"tool","tool_call_id":"call_b","content":"rain"},
        {"role":"tool","tool_call_id":"call_a","content":"sun"}
    ]}));
    assert_eq!(
        result["input"][0],
        json!({"type":"function_call","call_id":"call_a","name":"weather","arguments":"{\"city\":\"a\"}"})
    );
    assert_eq!(
        result["input"][2],
        json!({"type":"function_call_output","call_id":"call_b","output":"rain"})
    );
    assert_eq!(result["input"][3]["call_id"], "call_a");
}

#[test]
fn preserves_non_strict_tools_and_maps_named_choice_and_json_schema() {
    let result = convert(
        json!({"model":"model-a","messages":[{"role":"user","content":"hello"}],
            "tools":[{"type":"function","function":{"name":"f","parameters":{"type":"object","properties":{"optional":{"type":"string"}}}}}],
            "tool_choice":{"type":"function","function":{"name":"f"}},"parallel_tool_calls":true,"reasoning_effort":"high",
            "response_format":{"type":"json_schema","json_schema":{"name":"answer","strict":true,"schema":{"type":"object"}}}
        }),
    );
    assert_eq!(result["tools"][0]["strict"], false);
    assert!(result["tools"][0]["parameters"]["required"].is_null());
    assert_eq!(result["tool_choice"], json!({"type":"function","name":"f"}));
    assert_eq!(result["reasoning"], json!({"effort":"high"}));
    assert_eq!(
        result["text"]["format"],
        json!({"type":"json_schema","name":"answer","strict":true,"schema":{"type":"object"}})
    );
}

#[test]
fn refuses_parameters_that_cannot_be_honored_instead_of_silently_dropping_them() {
    for (field, value) in [
        ("temperature", json!(0.2)),
        ("max_tokens", json!(10)),
        ("max_completion_tokens", json!(10)),
        ("n", json!(2)),
        ("stop", json!(["END"])),
        ("logprobs", json!(true)),
        ("store", json!(true)),
        ("functions", json!([])),
        ("unknown", json!(true)),
    ] {
        let mut request = json!({"model":"model-a","messages":[{"role":"user","content":"hello"}]});
        request[field] = value;
        let error = decode_chat_request(request.to_string().as_bytes())
            .err()
            .expect("unsupported parameter");
        assert_eq!(error.param, field);
    }
}

#[test]
fn validates_shapes_before_execution_and_does_not_echo_prompt_values() {
    for request in [
        json!({}),
        json!({"model":"model-a","messages":[]}),
        json!({"model":"model-a","messages":[{"role":"tool","content":"secret prompt"}]}),
        json!({"model":"model-a","messages":[{"role":"user","content":[{"type":"input_audio","input_audio":{"data":"secret prompt"}}]}]}),
    ] {
        let error = decode_chat_request(request.to_string().as_bytes())
            .err()
            .expect("invalid request");
        assert!(!error.to_string().contains("secret prompt"));
    }
}

#[test]
fn streaming_usage_requires_streaming_and_boolean_options() {
    let mut request = json!({"model":"model-a","messages":[{"role":"user","content":"hello"}],"stream_options":{"include_usage":true}});
    assert!(decode_chat_request(request.to_string().as_bytes()).is_err());
    request["stream"] = json!(true);
    let decoded = decode_chat_request(request.to_string().as_bytes()).unwrap();
    assert!(decoded.stream && decoded.include_usage);
    request["stream_options"]["include_usage"] = json!("true");
    assert!(decode_chat_request(request.to_string().as_bytes()).is_err());
}

#[test]
fn rejects_misplaced_tool_fields_and_invalid_image_detail_instead_of_dropping_them() {
    for message in [
        json!({"role":"user","content":"hello","tool_call_id":"call_a"}),
        json!({"role":"tool","content":"result","tool_call_id":"call_a","tool_calls":[]}),
        json!({"role":"user","content":[{"type":"image_url","image_url":{"url":"https://example.invalid/a.png","detail":17}}]}),
    ] {
        let request = json!({"model":"model-a","messages":[message]});
        assert!(decode_chat_request(request.to_string().as_bytes()).is_err());
    }
    let result = convert(
        json!({"model":"model-a","messages":[{"role":"assistant","content":[{"type":"refusal","refusal":"cannot comply"}]}]}),
    );
    assert_eq!(
        result["input"][0]["content"],
        json!([{"type":"refusal","refusal":"cannot comply"}])
    );
}
