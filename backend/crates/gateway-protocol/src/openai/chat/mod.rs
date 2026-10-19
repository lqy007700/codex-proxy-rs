//! Chat Completions 请求与 Responses 输出的双向协议转换

mod request;
mod response;
mod stream;

pub use request::{ChatRequest, ChatRequestError, decode_chat_request};
pub use response::{ChatResponseError, chat_completion};
pub use stream::ChatStreamEncoder;
