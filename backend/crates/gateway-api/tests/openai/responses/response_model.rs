//! 客户端模型投影的原文保留、生命周期、错误与观测隔离回归

use super::{
    Bytes, OpenAiResponsesEncoder, ProtocolWireEvent, ProviderEvent, Value, json, parse_sse_events,
};

fn encoder() -> OpenAiResponsesEncoder {
    OpenAiResponsesEncoder::new().with_requested_model("client-alias".to_owned())
}

fn websocket_event(kind: &str, raw: &str) -> ProviderEvent {
    ProviderEvent::wire(
        ProtocolWireEvent::json(
            "openai",
            Some(kind.to_owned()),
            serde_json::from_str(raw).unwrap(),
        )
        .unwrap()
        .with_raw_websocket_message(raw.to_owned()),
    )
}

#[test]
fn websocket_model_projection_changes_only_known_string_spans() {
    let raw = r#"{ "type":"response.created", "response": {"id":"resp_model","model" : "upstream-model", "headers":{"OpenAI-Model":"upstream-header","x-openai-model":["upstream-array",7],"x-opaque":"keep"},"output":[{"model":"business-model","text":"upstream-model"}],"opaque":1.2300e+42},"headers":{"OPENAI-MODEL":"upstream-top"},"model":"business-root" }"#;
    let event = websocket_event("response.created", raw);
    let expected = raw.replacen("\"upstream-model\"", "\"client-alias\"", 1);
    let expected = expected
        .replace("\"upstream-header\"", "\"client-alias\"")
        .replace("\"upstream-array\"", "\"client-alias\"")
        .replace("\"upstream-top\"", "\"client-alias\"");
    assert_eq!(encoder().push_websocket(&event), vec![expected]);
    assert_eq!(
        event.wire_event().unwrap().raw_websocket_message(),
        Some(raw)
    );
    assert_eq!(
        event.wire_event().unwrap().data()["response"]["model"],
        "upstream-model"
    );
}

#[test]
fn sse_model_projection_preserves_multiline_framing_comments_and_numbers() {
    let raw = ": heartbeat\r\nid: opaque\r\nretry: 0042\r\nevent: response.created\r\ndata: {\"type\":\"response.created\",\r\ndata:  \"response\":{\"id\":\"resp_model\",\"model\":\"upstream\",\"headers\":{\"openai-model\":\"reported\"}},\"n\":1.200e+40}\r\n\r\n";
    let parsed = parse_sse_events(raw).unwrap();
    let event = ProviderEvent::wire(
        ProtocolWireEvent::json_with_raw_sse_metadata(
            "openai",
            Some("response.created".to_owned()),
            serde_json::from_str(&parsed[0].data).unwrap(),
            Bytes::copy_from_slice(raw.as_bytes()),
            Some("opaque".into()),
            Some(42),
        )
        .unwrap(),
    );
    let expected = raw
        .replace("\"upstream\"", "\"client-alias\"")
        .replace("\"reported\"", "\"client-alias\"");
    assert_eq!(encoder().push_sse(&event), vec![Bytes::from(expected)]);
}

#[test]
fn known_lifecycle_events_project_model_but_do_not_fabricate_missing_fields() {
    for kind in [
        "response.created",
        "response.queued",
        "response.in_progress",
        "response.completed",
        "response.incomplete",
        "response.failed",
    ] {
        for model in [
            Some(json!("upstream")),
            None,
            Some(Value::Null),
            Some(json!(42)),
        ] {
            let mut value = json!({"type":kind,"response":{"id":"resp_model","output":[]}});
            if let Some(model) = &model {
                value["response"]["model"] = model.clone();
            }
            let event = websocket_event(kind, &value.to_string());
            let delivered: Value =
                serde_json::from_str(&encoder().push_websocket(&event)[0]).unwrap();
            let mut expected = value;
            if model.as_ref().is_some_and(Value::is_string) {
                expected["response"]["model"] = json!("client-alias");
            }
            assert_eq!(delivered, expected, "{kind}");
        }
    }
}

#[test]
fn metadata_projects_model_headers_and_keeps_business_metadata() {
    for kind in ["response.metadata", "codex.response.metadata"] {
        let raw = json!({"type":kind,"headers":{"OpenAI-Model":"upstream","X-OpenAI-Model":["upstream",null],"x-request-id":"req_original"},"metadata":{"model":"business-model"}}).to_string();
        let event = websocket_event(kind, &raw);
        let delivered: Value = serde_json::from_str(&encoder().push_websocket(&event)[0]).unwrap();
        assert_eq!(delivered["headers"]["OpenAI-Model"], "client-alias");
        assert_eq!(
            delivered["headers"]["X-OpenAI-Model"],
            json!(["client-alias", null])
        );
        assert_eq!(delivered["metadata"]["model"], "business-model");
        assert_eq!(delivered["headers"]["x-request-id"], "req_original");
    }
}

#[test]
fn unknown_events_and_identical_models_keep_raw_bytes() {
    for (kind, raw) in [
        (
            "response.future",
            r#"{ "type":"response.future", "response":{"model":"upstream"}, "headers":{"openai-model":"upstream"} }"#,
        ),
        (
            "response.created",
            r#"{ "type":"response.created", "response":{"id":"resp_model","model":"client-alias"}, "n":1.200e+42 }"#,
        ),
        (
            "response.output_item.done",
            r#"{ "type":"response.output_item.done", "item":{"model":"upstream"} }"#,
        ),
    ] {
        assert_eq!(
            encoder().push_websocket(&websocket_event(kind, raw)),
            vec![raw.to_owned()]
        );
    }
}

#[test]
fn model_projection_keeps_unparseable_wire_transparent() {
    for raw in [
        ": heartbeat\r\n\r\n",
        "event: response.created\ndata: future non-JSON payload\n\n",
        "event: response.future\ndata: {malformed\n\n",
    ] {
        let bytes = Bytes::copy_from_slice(raw.as_bytes());
        let event =
            ProviderEvent::wire(ProtocolWireEvent::raw_sse("openai", bytes.clone()).unwrap());
        assert_eq!(encoder().push_sse(&event), vec![bytes]);
        assert!(!event.middleware_transformed());
    }
    for raw in [
        "future non-JSON payload",
        r#"{"type":"response.created","response":invalid}"#,
    ] {
        let event = ProviderEvent::wire(ProtocolWireEvent::raw_websocket("openai", raw).unwrap());
        assert_eq!(encoder().push_websocket(&event), vec![raw.to_owned()]);
        assert!(!event.middleware_transformed());
    }
}

#[test]
fn error_projection_uses_client_model_without_changing_upstream_error() {
    let mut encoder = encoder();
    encoder.push_websocket(&websocket_event(
        "response.created",
        r#"{"type":"response.created","response":{"id":"resp_model","model":"upstream"}}"#,
    ));
    let raw = r#"{"type":"error","error":{"code":"custom_error","message":"upstream-model","model":"business-error"}}"#;
    let event = websocket_event("error", raw);
    let delivered: Value = serde_json::from_str(&encoder.push_websocket(&event)[0]).unwrap();
    assert_eq!(delivered["type"], "response.failed");
    assert_eq!(delivered["response"]["model"], "client-alias");
    assert_eq!(
        delivered["response"]["error"],
        json!({"code":"custom_error","message":"upstream-model","model":"business-error"})
    );
    assert_eq!(
        event.wire_event().unwrap().raw_websocket_message(),
        Some(raw)
    );
}

#[test]
fn client_model_is_json_escaped_and_terminal_snapshot_is_projected() {
    let name = "客户端\"\\\n\0model";
    let mut encoder = OpenAiResponsesEncoder::new().with_requested_model(name.to_owned());
    let event = websocket_event(
        "response.completed",
        r#"{"type":"response.completed","response":{"id":"resp_model","model":"upstream","status":"completed","output":[],"usage":{"total_tokens":7}}}"#,
    );
    let delivered: Value = serde_json::from_str(&encoder.push_websocket(&event)[0]).unwrap();
    assert_eq!(delivered["response"]["model"], name);
    assert_eq!(encoder.finish().unwrap()["model"], name);
    assert_eq!(
        event.wire_event().unwrap().data()["response"]["usage"]["total_tokens"],
        7
    );
}

#[test]
fn openai_message_start_extensions_are_not_anthropic_projection_targets() {
    for kind in ["message_start", "anthropic.message_start"] {
        let raw = format!(
            r#"{{ "type":"{kind}", "message":{{"model":"upstream"}}, "model":"business-root" }}"#
        );
        let event = websocket_event(kind, &raw);
        assert_eq!(encoder().push_websocket(&event), vec![raw]);
    }
}
