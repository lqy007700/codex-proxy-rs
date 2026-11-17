//! 请求诊断与操作故障的观测合同，不参与路由、重试或客户端协议决策

mod capture;
mod failure;
mod privacy;
mod retry;
mod selection;
mod stream;
mod trace;

pub use capture::{body_fingerprint, diagnostic_headers, diagnostic_json, json_fingerprint};
pub use failure::{OperationalDiagnostics, OperationalFailure};
pub use privacy::{DiagnosticRedactionReport, DiagnosticRedactor};
pub use retry::MAX_RETRY_REQUEST_BODY_BYTES;
pub use stream::{StreamCapture, StreamFormat};
pub use trace::{MAX_TRACE_SNAPSHOT_BYTES, TraceContext};
