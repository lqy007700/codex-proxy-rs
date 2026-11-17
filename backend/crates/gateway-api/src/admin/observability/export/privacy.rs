//! 诊断下载应用共享脱敏规则并汇总导出缺口

use gateway_core::diagnostics::{DiagnosticRedactor, MAX_RETRY_REQUEST_BODY_BYTES};
use serde_json::{Value, json};

pub(super) fn redact(mut value: Value) -> Value {
    let mut redactor = DiagnosticRedactor::new(8 * 1024 * 1024, 256 * 1024);
    // 先处理有界文件信息，正文耗尽预算也不能删除 schema、目标或既有缺口
    let mut error = value["error"].take();
    let mut requests = value["requests"].take();
    redactor.redact(&mut value);
    redactor.redact(&mut error);
    if let Some(requests) = requests.as_array_mut() {
        for request in requests {
            let mut retry_body = request
                .pointer_mut("/evidence/trace/retryRequestBody")
                .map(Value::take);
            redactor.redact(request);
            if let Some(body) = &mut retry_body {
                redactor.redact_preserving_json_strings(body, MAX_RETRY_REQUEST_BODY_BYTES);
                if let Some(trace) = request
                    .pointer_mut("/evidence/trace")
                    .and_then(Value::as_object_mut)
                {
                    trace.insert("retryRequestBody".to_owned(), body.take());
                }
            }
        }
    }
    value["error"] = error;
    value["requests"] = requests;
    if redactor.report().truncated_values > 0 {
        // 缺口清单可能也触及预算，顶层统计始终保留以免截断变成静默丢失
        value["exportTruncated"] = json!(true);
        if !value["availability"]["gaps"].is_array() {
            value["availability"] = json!({"gaps": []});
        }
        value["availability"]["gaps"]
            .as_array_mut()
            .expect("gap array")
            .push(json!({
                "section": "export", "reason": "export_size_limit",
            }));
    }
    value["redaction"] = json!(redactor.report());
    value
}
