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
fn maps_completion_token_budget_to_responses_output_budget() {
    for field in ["max_tokens", "max_completion_tokens"] {
        for stream in [false, true] {
            let mut request = json!({"model":"model-a","messages":[{"role":"user","content":"hello"}],"stream":stream});
            request[field] = json!(1024);
            let result = convert(request);
            assert_eq!(result["max_output_tokens"], 1024);
            assert!(result.get(field).is_none());
        }
    }
}

#[test]
fn newer_token_budget_overrides_legacy_budget_and_null_keeps_legacy_value() {
    let mut request = json!({"model":"model-a","messages":[{"role":"user","content":"hello"}],"max_tokens":128,"max_completion_tokens":256});
    assert_eq!(convert(request.clone())["max_output_tokens"], 256);
    request["max_completion_tokens"] = Value::Null;
    assert_eq!(convert(request)["max_output_tokens"], 128);
}

#[test]
fn preserves_known_sampling_cache_and_moderation_parameters_with_text_format() {
    let request = json!({
        "model":"model-a","messages":[{"role":"user","content":"hello"}],
        "temperature":0.2,"top_p":0.8,"verbosity":"low",
        "response_format":{"type":"json_object"},
        "prompt_cache_key":"cache","prompt_cache_retention":"24h",
        "prompt_cache_options":{"enabled":true},"moderation":{"enabled":true},
        "user":"synthetic-user","metadata":{"purpose":"test"},"safety_identifier":"synthetic-id"
    });
    let result = convert(request.clone());
    assert_eq!(
        result["text"],
        json!({"verbosity":"low","format":{"type":"json_object"}})
    );
    for field in [
        "temperature",
        "top_p",
        "prompt_cache_key",
        "prompt_cache_retention",
        "prompt_cache_options",
        "moderation",
        "user",
        "metadata",
        "safety_identifier",
    ] {
        assert_eq!(result[field], request[field], "{field}");
    }
    assert_eq!(
        convert(
            json!({"model":"model-a","messages":[{"role":"user","content":"hello"}],"verbosity":"high"})
        )["text"],
        json!({"verbosity":"high"})
    );
}

#[test]
fn rejects_malformed_known_parameters_before_execution() {
    for (field, value) in [
        ("max_tokens", json!(0)),
        ("max_tokens", json!("100")),
        ("max_tokens", json!(1.5)),
        ("temperature", json!(-0.1)),
        ("temperature", json!(2.1)),
        ("temperature", json!("1")),
        ("top_p", json!(1.1)),
        ("top_p", json!(-0.1)),
        ("top_p", json!(true)),
        ("verbosity", json!(1)),
        ("user", json!({})),
        ("prompt_cache_retention", json!(false)),
        ("prompt_cache_options", json!([])),
        ("moderation", json!("enabled")),
        ("metadata", json!([])),
    ] {
        let mut request = json!({"model":"model-a","messages":[{"role":"user","content":"hello"}]});
        request[field] = value;
        let error = decode_chat_request(request.to_string().as_bytes())
            .err()
            .expect("invalid shape or range");
        assert_eq!(error.param, field);
    }
    for (temperature, top_p) in [(0, 0), (2, 1)] {
        let result = convert(
            json!({"model":"model-a","messages":[{"role":"user","content":"hello"}],"temperature":temperature,"top_p":top_p}),
        );
        assert_eq!(result["temperature"], temperature);
        assert_eq!(result["top_p"], top_p);
    }
}
#[test]
fn null_completion_token_budget_keeps_the_provider_default() {
    let result = convert(
        json!({"model":"model-a","messages":[{"role":"user","content":"hello"}],"max_completion_tokens":null}),
    );
    assert!(result.get("max_output_tokens").is_none());
}

#[test]
fn rejects_invalid_completion_token_budgets_with_the_chat_parameter_name() {
    for value in [
        json!(0),
        json!(-1),
        json!(1.5),
        json!("1024"),
        json!(true),
        json!([]),
        json!({}),
    ] {
        let request = json!({"model":"model-a","messages":[{"role":"user","content":"hello"}],"max_completion_tokens":value});
        let error = decode_chat_request(request.to_string().as_bytes())
            .err()
            .expect("invalid token budget");
        assert_eq!(error.param, "max_completion_tokens");
        assert_eq!(
            error.message,
            "max_completion_tokens must be a positive integer."
        );
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
