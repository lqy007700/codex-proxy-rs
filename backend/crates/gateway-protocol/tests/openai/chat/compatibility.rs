//! 扩展 Chat 请求与真实 Responses 输出的兼容合同

use gateway_protocol::openai::chat::{ChatStreamEncoder, chat_completion, decode_chat_request};
use serde_json::{Value, json};

use super::response::response;

fn convert(value: Value) -> Value {
    Value::Object(
        decode_chat_request(value.to_string().as_bytes())
            .unwrap()
            .responses,
    )
}

#[test]
fn preserves_custom_tools_files_audio_and_native_responses_options() {
    let body = convert(json!({"model":"alias","messages":[
        {"role":"user","content":[{"type":"file","file":{"file_id":"file_synthetic"}},
            {"type":"input_audio","input_audio":{"data":"aGk=","format":"mp3"}}]},
        {"role":"assistant","tool_calls":[{"id":"call_patch","type":"custom","custom":{"name":"patch","input":"raw\ninput"}}]},
        {"role":"tool","tool_call_id":"call_patch","content":"done"}],
        "tools":[{"type":"custom","custom":{"name":"patch"}}],
        "tool_choice":{"type":"custom","custom":{"name":"patch"}},
        "reasoning":{"summary":"auto"},"reasoning_effort":"high",
        "max_output_tokens":64,"max_tokens":128,"max_completion_tokens":256,
        "logprobs":true,"top_logprobs":3,"stop":["END"],"seed":42,"client_extension":true}));
    assert_eq!(
        body["input"][0]["content"][0],
        json!({"type":"input_file","file_id":"file_synthetic"})
    );
    assert_eq!(
        body["input"][0]["content"][1],
        json!({"type":"input_audio","audio_url":"data:audio/mpeg;base64,aGk="})
    );
    assert_eq!(body["input"][1]["type"], "custom_tool_call");
    assert_eq!(body["input"][1]["input"], "raw\ninput");
    assert_eq!(body["input"][2]["type"], "custom_tool_call_output");
    assert_eq!(body["tools"][0], json!({"type":"custom","name":"patch"}));
    assert_eq!(body["reasoning"], json!({"summary":"auto","effort":"high"}));
    assert_eq!(body["max_output_tokens"], 256);
    assert_eq!(body["include"], json!(["message.output_text.logprobs"]));
    assert_eq!(body["top_logprobs"], 3);
    for field in ["stop", "seed", "client_extension"] {
        assert!(body.get(field).is_none());
    }
}

#[test]
fn search_options_flatten_location_and_reject_conflicting_tool_selection() {
    let mut request = json!({"model":"alias","messages":[{"role":"user","content":"search"}],
        "web_search_options":{"search_context_size":"low","user_location":{"type":"approximate","approximate":{"country":"US","city":"Seattle"}}}});
    let body = convert(request.clone());
    assert_eq!(
        body["tools"][0],
        json!({"type":"web_search","search_context_size":"low","user_location":{"type":"approximate","country":"US","city":"Seattle"}})
    );
    assert_eq!(body["tool_choice"], json!({"type":"web_search"}));
    request["tools"] = json!([{"type":"function","function":{"name":"f"}}]);
    assert_eq!(
        decode_chat_request(request.to_string().as_bytes())
            .err()
            .unwrap()
            .param,
        "tools"
    );
}

#[test]
fn json_and_stream_project_public_reasoning_images_custom_calls_and_citations() {
    let outputs = json!([
        {"id":"rs_public","type":"reasoning","summary":[{"type":"summary_text","text":"public summary"}],"encrypted_content":"synthetic-private-cipher"},
        {"id":"msg_a","type":"message","role":"assistant","content":[{"type":"output_text","text":"link","logprobs":[{"token":"link","logprob":-0.1}],"annotations":[{"type":"url_citation","url":"https://example.invalid","title":"source","start_index":0,"end_index":4}]}]},
        {"id":"img_a","type":"image_generation_call","result":"aGk=","output_format":"png"},
        {"id":"ct_a","type":"custom_tool_call","call_id":"call_patch","name":"patch","input":"raw"}
    ]);
    let wire = response(outputs);
    let result = chat_completion(&wire, "public-alias").unwrap();
    let message = &result["choices"][0]["message"];
    assert_eq!(message["content"], "link");
    assert_eq!(message["reasoning_content"], "public summary");
    assert_eq!(
        message["images"][0]["image_url"]["url"],
        "data:image/png;base64,aGk="
    );
    assert_eq!(message["tool_calls"][0]["custom"]["input"], "raw");
    assert_eq!(message["annotations"][0]["url_citation"]["end_index"], 4);
    assert_eq!(
        result["choices"][0]["logprobs"]["content"][0]["token"],
        "link"
    );
    assert!(!result.to_string().contains("synthetic-private-cipher"));
    let mut stream = ChatStreamEncoder::new("public-alias".into(), 0, true);
    stream
        .push(
            "response.created",
            &json!({"response":{"id":"resp_synthetic","created_at":123}}),
        )
        .unwrap();
    let frames = stream
        .push("response.completed", &json!({"response":wire}))
        .unwrap();
    assert!(frames.iter().all(|frame| frame["model"] == "public-alias"));
    assert!(
        frames
            .iter()
            .any(|frame| frame["choices"][0]["delta"]["reasoning_content"] == "public summary")
    );
    assert!(
        frames
            .iter()
            .any(|frame| frame["choices"][0]["delta"]["images"].is_array())
    );
    assert_eq!(frames.last().unwrap()["usage"]["total_tokens"], 17);
    assert!(
        !serde_json::to_string(&frames)
            .unwrap()
            .contains("synthetic-private-cipher")
    );
}

#[test]
fn streaming_requires_identity_and_bounds_output_state() {
    let mut stream = ChatStreamEncoder::new("alias".into(), 0, false);
    assert!(
        stream
            .push(
                "response.output_text.delta",
                &json!({"output_index":0,"content_index":0,"delta":"unbound"})
            )
            .is_err()
    );
    let mut stream = ChatStreamEncoder::new("alias".into(), 0, false);
    stream
        .push(
            "response.created",
            &json!({"response":{"id":"resp_synthetic"}}),
        )
        .unwrap();
    for index in 0..4095 {
        stream
            .push(
                "response.output_text.delta",
                &json!({"output_index":0,"content_index":index,"delta":"x"}),
            )
            .unwrap();
    }
    assert!(
        stream
            .push(
                "response.output_text.delta",
                &json!({"output_index":0,"content_index":4095,"delta":"x"})
            )
            .is_err()
    );
}

#[test]
fn completed_images_require_payload_but_in_progress_images_may_be_empty() {
    for image in [
        json!({"id":"img_a","type":"image_generation_call"}),
        json!({"id":"img_a","type":"image_generation_call","result":null}),
        json!({"id":"img_a","type":"image_generation_call","result":""}),
    ] {
        let completion = response(json!([image]));
        assert!(chat_completion(&completion, "alias").is_err());
        let mut stream = ChatStreamEncoder::new("alias".into(), 0, false);
        stream
            .push(
                "response.created",
                &json!({"response":{"id":"resp_synthetic","created_at":123}}),
            )
            .unwrap();
        assert!(
            stream
                .push(
                    "response.output_item.added",
                    &json!({"output_index":0,"item":image})
                )
                .is_ok()
        );
        assert!(
            stream
                .push("response.completed", &json!({"response":completion}))
                .is_err()
        );
    }
}

#[test]
fn tool_deltas_that_contradict_final_snapshot_are_not_reported_as_success() {
    let item = json!({"type":"function_call","id":"fc_internal","call_id":"call_client","name":"f","arguments":"{}"});
    let mut stream = ChatStreamEncoder::new("alias".into(), 0, false);
    stream
        .push(
            "response.created",
            &json!({"response":{"id":"resp_synthetic","created_at":123}}),
        )
        .unwrap();
    stream
        .push(
            "response.output_item.added",
            &json!({"output_index":0,"item":item}),
        )
        .unwrap();
    stream
        .push(
            "response.function_call_arguments.delta",
            &json!({"item_id":"fc_internal","delta":"{}"}),
        )
        .unwrap();
    assert!(
        stream
            .push(
                "response.completed",
                &json!({"response":response(json!([item]))})
            )
            .is_err()
    );
}

#[test]
fn image_done_requires_a_complete_result_even_without_an_added_event() {
    for result in [None, Some(Value::Null), Some(json!(""))] {
        for added in [false, true] {
            let mut image = json!({"id":"img_a","type":"image_generation_call"});
            if let Some(result) = &result {
                image["result"] = result.clone();
            }
            let mut stream = ChatStreamEncoder::new("alias".into(), 0, false);
            stream
                .push(
                    "response.created",
                    &json!({"response":{"id":"resp_synthetic","created_at":123}}),
                )
                .unwrap();
            if added {
                assert!(
                    stream
                        .push(
                            "response.output_item.added",
                            &json!({"output_index":0,"item":image})
                        )
                        .is_ok()
                );
            }
            assert!(
                stream
                    .push(
                        "response.output_item.done",
                        &json!({"output_index":0,"item":image})
                    )
                    .is_err()
            );
            assert!(!stream.is_terminal());
        }
    }
}

#[test]
fn omitted_or_empty_terminal_output_cannot_turn_a_preview_into_a_completed_image() {
    for progress in ["empty_added", "populated_added", "partial_image"] {
        for omitted in [false, true] {
            let mut stream = ChatStreamEncoder::new("alias".into(), 0, false);
            stream
                .push(
                    "response.created",
                    &json!({"response":{"id":"resp_synthetic","created_at":123}}),
                )
                .unwrap();
            if progress == "partial_image" {
                stream
                    .push(
                        "response.image_generation_call.partial_image",
                        &json!({"output_index":0,"item_id":"img_a","partial_image_b64":"aGk="}),
                    )
                    .unwrap();
            } else {
                let mut image = json!({"id":"img_a","type":"image_generation_call"});
                if progress == "populated_added" {
                    image["result"] = json!("aGk=");
                }
                stream
                    .push(
                        "response.output_item.added",
                        &json!({"output_index":0,"item":image}),
                    )
                    .unwrap();
            }
            let mut completion = response(json!([]));
            if omitted {
                completion.as_object_mut().unwrap().remove("output");
            }
            assert!(
                stream
                    .push("response.completed", &json!({"response":completion}))
                    .is_err()
            );
            assert!(!stream.is_terminal());
        }
    }
}

#[test]
fn completed_images_survive_omitted_terminal_output_without_duplicate_delivery() {
    for progress in ["done_only", "empty_added", "partial_preview"] {
        for final_output in ["omitted", "empty", "full"] {
            let image = json!({"id":"img_a","type":"image_generation_call","result":"aGk=","output_format":"png"});
            let mut stream = ChatStreamEncoder::new("alias".into(), 0, false);
            let mut delivered = stream
                .push(
                    "response.created",
                    &json!({"response":{"id":"resp_synthetic","created_at":123}}),
                )
                .unwrap();
            if progress == "empty_added" {
                delivered.extend(stream.push(
                    "response.output_item.added",
                    &json!({"output_index":0,"item":{"id":"img_a","type":"image_generation_call"}}),
                ).unwrap());
            } else if progress == "partial_preview" {
                delivered.extend(
                    stream
                        .push(
                            "response.image_generation_call.partial_image",
                            &json!({"output_index":0,"item_id":"img_a","partial_image_b64":"aGk="}),
                        )
                        .unwrap(),
                );
            }
            delivered.extend(
                stream
                    .push(
                        "response.output_item.done",
                        &json!({"output_index":0,"item":image}),
                    )
                    .unwrap(),
            );
            let mut completion = response(if final_output == "full" {
                json!([image])
            } else {
                json!([])
            });
            if final_output == "omitted" {
                completion.as_object_mut().unwrap().remove("output");
            }
            delivered.extend(
                stream
                    .push("response.completed", &json!({"response":completion}))
                    .unwrap(),
            );
            assert!(stream.is_terminal());
            assert_eq!(
                delivered
                    .iter()
                    .filter(|frame| frame.pointer("/choices/0/delta/images").is_some())
                    .count(),
                1
            );
            assert_eq!(
                delivered
                    .iter()
                    .filter(
                        |frame| frame.pointer("/choices/0/finish_reason") == Some(&json!("stop"))
                    )
                    .count(),
                1
            );
        }
    }
}
