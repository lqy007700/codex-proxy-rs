//! 客户端响应模型名称投影，不改变 Provider 原始事件和执行观测

use std::{collections::BTreeMap, ops::Range};

use bytes::Bytes;
use gateway_core::engine::middleware::{MiddlewareHeader, MiddlewareResponse};
use http::HeaderValue;
use serde::Deserialize;
use serde_json::{Value, value::RawValue};

/// 每次逻辑请求独立冻结的客户端模型名称
#[derive(Debug)]
pub(super) struct ClientResponseModel(String);

#[derive(Deserialize)]
struct ModelFields<'a> {
    #[serde(borrow)]
    model: Option<&'a RawValue>,
    #[serde(borrow)]
    response: Option<&'a RawValue>,
    #[serde(borrow)]
    headers: Option<&'a RawValue>,
}

type Replacement = (Range<usize>, String);

impl ClientResponseModel {
    pub(super) fn new(model: String) -> Self {
        Self(model)
    }

    pub(super) fn as_str(&self) -> &str {
        &self.0
    }

    pub(super) fn project_headers(&self, headers: &mut Vec<MiddlewareHeader>) {
        // 请求模型名是 opaque 字符串，不能为了 HTTP 头收紧正文合同
        let replacement = HeaderValue::from_str(&self.0).ok();
        headers.retain_mut(|header| {
            if !is_model_header(header.name()) {
                return true;
            }
            let Some(value) = replacement.as_ref() else {
                return false;
            };
            *header =
                MiddlewareHeader::new(header.name(), Bytes::copy_from_slice(value.as_bytes()));
            true
        });
    }

    pub(super) fn project_response_headers(
        &self,
        response: MiddlewareResponse,
    ) -> MiddlewareResponse {
        let (protocol, status, mut headers, body, envelope) = response.into_parts();
        self.project_headers(&mut headers);
        let mut response = MiddlewareResponse::new(protocol, status, headers, body);
        if let Some(envelope) = envelope {
            response = response.with_envelope(envelope);
        }
        response
    }

    pub(super) fn project_response(&self, response: &mut Value) {
        if let Some(model) = response.get_mut("model").filter(|value| value.is_string()) {
            *model = Value::String(self.0.clone());
        }
        if let Some(headers) = response.get_mut("headers").and_then(Value::as_object_mut) {
            for (name, value) in headers {
                if is_model_header(name) {
                    self.project_header_value(value);
                }
            }
        }
    }

    fn project_header_value(&self, value: &mut Value) {
        match value {
            Value::String(_) => *value = Value::String(self.0.clone()),
            Value::Array(values) => {
                for value in values {
                    if value.is_string() {
                        *value = Value::String(self.0.clone());
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn project_event(&self, kind: &str, value: &mut Value) {
        if is_response_event(kind)
            && let Some(response) = value.get_mut("response")
        {
            self.project_response(response);
        }
        if (is_response_event(kind) || is_metadata_event(kind))
            && let Some(headers) = value.get_mut("headers").and_then(Value::as_object_mut)
        {
            for (name, value) in headers {
                if is_model_header(name) {
                    self.project_header_value(value);
                }
            }
        }
    }

    pub(super) fn project_json(&self, json: &str, kind: &str) -> Option<String> {
        let replacements = self.replacements(json, kind)?;
        Some(apply_replacements(json, replacements))
    }

    pub(super) fn project_sse(&self, raw: &Bytes, kind: &str) -> Option<Bytes> {
        if !is_response_event(kind) && !is_metadata_event(kind) {
            return None;
        }
        let raw = std::str::from_utf8(raw).ok()?;
        let mut data = String::new();
        let mut segments = Vec::new();
        for (index, line) in raw.split_inclusive('\n').enumerate() {
            let line = line.strip_suffix('\n').unwrap_or(line);
            let line = line.strip_suffix('\r').unwrap_or(line);
            // SSE 只删除冒号后的一个空格；JSON 中插入的换行用于保持多行 data 语义
            let line = if index == 0 {
                line.strip_prefix('\u{feff}').unwrap_or(line)
            } else {
                line
            };
            let Some(value) = line.strip_prefix("data:") else {
                continue;
            };
            let value = value.strip_prefix(' ').unwrap_or(value);
            if !segments.is_empty() {
                data.push('\n');
            }
            let start = data.len();
            data.push_str(value);
            segments.push((
                start..data.len(),
                value.as_ptr() as usize - raw.as_ptr() as usize,
            ));
        }
        let replacements = self.replacements(&data, kind)?;
        let mut mapped = Vec::with_capacity(replacements.len());
        for (range, replacement) in replacements {
            // 合法 JSON 字符串不会跨越 data 行；仅替换值的原始跨度，保留注释和 framing
            let (segment, offset) = segments
                .iter()
                .find(|(segment, _)| segment.start <= range.start && range.end <= segment.end)?;
            mapped.push((
                offset + range.start - segment.start..offset + range.end - segment.start,
                replacement,
            ));
        }
        Some(Bytes::from(apply_replacements(raw, mapped)))
    }

    fn replacements(&self, json: &str, kind: &str) -> Option<Vec<Replacement>> {
        if !is_response_event(kind) && !is_metadata_event(kind) {
            return None;
        }
        let fields: ModelFields<'_> = serde_json::from_str(json).ok()?;
        let mut replacements = Vec::new();
        if is_response_event(kind)
            && let Some(response) = fields.response
            && let Ok(response) = serde_json::from_str::<ModelFields<'_>>(response.get())
        {
            if let Some(model) = response.model {
                self.replace_string(json, model, &mut replacements);
            }
            self.replace_headers(json, response.headers, &mut replacements);
        }
        self.replace_headers(json, fields.headers, &mut replacements);
        (!replacements.is_empty()).then_some(replacements)
    }

    fn replace_headers(
        &self,
        json: &str,
        headers: Option<&RawValue>,
        replacements: &mut Vec<Replacement>,
    ) {
        let Some(headers) = headers.and_then(|headers| {
            serde_json::from_str::<BTreeMap<String, &RawValue>>(headers.get()).ok()
        }) else {
            return;
        };
        for (name, value) in headers {
            if !is_model_header(&name) {
                continue;
            }
            self.replace_string(json, value, replacements);
            if let Ok(values) = serde_json::from_str::<Vec<&RawValue>>(value.get()) {
                for value in values {
                    self.replace_string(json, value, replacements);
                }
            }
        }
    }

    fn replace_string(&self, json: &str, value: &RawValue, replacements: &mut Vec<Replacement>) {
        if let Ok(original) = serde_json::from_str::<String>(value.get())
            && original != self.0
        {
            let start = value.get().as_ptr() as usize - json.as_ptr() as usize;
            replacements.push((
                start..start + value.get().len(),
                Value::String(self.0.clone()).to_string(),
            ));
        }
    }
}

fn is_model_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("openai-model") || name.eq_ignore_ascii_case("x-openai-model")
}

fn is_response_event(kind: &str) -> bool {
    matches!(
        kind,
        "response.created"
            | "response.queued"
            | "response.in_progress"
            | "response.completed"
            | "response.incomplete"
            | "response.failed"
    )
}

fn is_metadata_event(kind: &str) -> bool {
    matches!(kind, "response.metadata" | "codex.response.metadata")
}

fn apply_replacements(original: &str, mut replacements: Vec<Replacement>) -> String {
    replacements.sort_unstable_by_key(|(range, _)| range.start);
    let mut result = String::with_capacity(original.len());
    let mut cursor = 0;
    for (range, value) in replacements {
        result.push_str(&original[cursor..range.start]);
        result.push_str(&value);
        cursor = range.end;
    }
    result.push_str(&original[cursor..]);
    result
}
