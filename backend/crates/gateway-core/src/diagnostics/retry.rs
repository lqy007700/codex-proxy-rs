//! 第二次尝试的请求正文使用独立预算，保存前移除可识别凭据

use serde_json::{Value, json};

use super::{DiagnosticRedactor, body_fingerprint};

pub const MAX_RETRY_REQUEST_BODY_BYTES: usize = 2 * 1024 * 1024;

pub(super) fn capture(bytes: &[u8], exchange_id: Option<u64>) -> Value {
    let mut result = json!({
        "attemptIndex": 2, "exchangeId": exchange_id,
        "body": body_fingerprint(bytes), "containsUserContent": true,
    });
    if bytes.len() > MAX_RETRY_REQUEST_BODY_BYTES {
        result["omitted"] = json!("request_body_size_limit");
        return result;
    }
    let Ok(mut body) = serde_json::from_slice::<Value>(bytes) else {
        result["omitted"] = json!("request_body_not_json");
        return result;
    };
    let mut redactor =
        DiagnosticRedactor::new(MAX_RETRY_REQUEST_BODY_BYTES, MAX_RETRY_REQUEST_BODY_BYTES);
    redactor.redact_preserving_json_strings(&mut body, MAX_RETRY_REQUEST_BODY_BYTES);
    result["redaction"] = json!(redactor.report());
    result["truncated"] = json!(redactor.report().truncated_values > 0);
    result["json"] = body;
    // 脱敏展开嵌套 JSON 后仍按最终编码限制存储，不能挤占时间线的预算
    if result.to_string().len() > MAX_RETRY_REQUEST_BODY_BYTES {
        result
            .as_object_mut()
            .expect("capture object")
            .remove("json");
        result["omitted"] = json!("request_body_size_limit");
    }
    result
}
