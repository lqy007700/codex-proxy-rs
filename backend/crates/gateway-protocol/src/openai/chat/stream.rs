//! Responses 事件到单候选 Chat 增量的有界状态转换

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::response::{ChatResponseError, chat_usage, finish_reason};

const MAX_OUTPUT_ITEMS: usize = 4096;

struct ToolState {
    index: usize,
    call_id: String,
    name: String,
    arguments_sent: bool,
}

/// 单次流的输出身份与工具索引；不缓存生成正文
pub struct ChatStreamEncoder {
    id: Option<String>,
    model: String,
    created: u64,
    include_usage: bool,
    terminal: bool,
    tools: BTreeMap<String, ToolState>,
    content: BTreeSet<(u64, u64)>,
}

impl ChatStreamEncoder {
    #[must_use]
    pub fn new(model: String, created: u64, include_usage: bool) -> Self {
        Self {
            id: None,
            model,
            created,
            include_usage,
            terminal: false,
            tools: BTreeMap::new(),
            content: BTreeSet::new(),
        }
    }

    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        self.terminal
    }

    /// 消费完整事件；失败事件保留 error 对象，不生成成功终态
    pub fn push(&mut self, kind: &str, data: &Value) -> Result<Vec<Value>, ChatResponseError> {
        if self.terminal {
            return Err(ChatResponseError);
        }
        let mut chunks = Vec::new();
        if let Some(response) = data.get("response")
            && let Some(id) = response.get("id").and_then(Value::as_str)
        {
            if self.id.as_deref().is_some_and(|previous| previous != id) {
                return Err(ChatResponseError);
            }
            if self.id.is_none() {
                self.id = Some(id.to_owned());
                if let Some(created) = response.get("created_at").and_then(Value::as_u64) {
                    self.created = created;
                }
                if let Some(model) = response.get("model").and_then(Value::as_str) {
                    self.model = model.to_owned();
                }
                chunks.push(self.chunk(json!({"role": "assistant", "content": ""}), Value::Null)?);
            }
        }

        match kind {
            "response.created" | "response.in_progress" | "response.queued" => {
                self.identity()?;
            }
            "response.output_text.delta" | "response.refusal.delta" => {
                let key = content_key(data)?;
                self.track_content(key)?;
                let delta = data
                    .get("delta")
                    .and_then(Value::as_str)
                    .ok_or(ChatResponseError)?;
                chunks.push(self.content_chunk(kind.contains("refusal"), delta)?);
            }
            "response.output_text.done" | "response.refusal.done" => {
                let key = content_key(data)?;
                if !self.content.contains(&key) {
                    self.track_content(key)?;
                    let refusal = kind.contains("refusal");
                    let text = data
                        .get(if refusal { "refusal" } else { "text" })
                        .and_then(Value::as_str)
                        .ok_or(ChatResponseError)?;
                    chunks.push(self.content_chunk(refusal, text)?);
                }
            }
            "response.output_item.added" | "response.output_item.done" => {
                let item = data.get("item").ok_or(ChatResponseError)?;
                if kind.ends_with("done") {
                    let index = data
                        .get("output_index")
                        .and_then(Value::as_u64)
                        .ok_or(ChatResponseError)?;
                    chunks.extend(self.terminal_item(index, item)?);
                } else if item.get("type").and_then(Value::as_str) == Some("function_call") {
                    chunks.extend(self.tool_item(item, kind.ends_with("done"))?);
                }
            }
            "response.function_call_arguments.delta" | "response.function_call_arguments.done" => {
                let item_id = data
                    .get("item_id")
                    .and_then(Value::as_str)
                    .ok_or(ChatResponseError)?;
                let tool = self.tools.get_mut(item_id).ok_or(ChatResponseError)?;
                let done = kind.ends_with("done");
                if !done || !tool.arguments_sent {
                    let arguments = data
                        .get(if done { "arguments" } else { "delta" })
                        .and_then(Value::as_str)
                        .ok_or(ChatResponseError)?;
                    tool.arguments_sent = true;
                    let index = tool.index;
                    chunks.push(self.chunk(json!({"tool_calls": [{"index": index, "function": {"arguments": arguments}}]}), Value::Null)?);
                }
            }
            "response.completed" | "response.incomplete" => {
                let response = data.get("response").ok_or(ChatResponseError)?;
                if let Some(output) = response.get("output").and_then(Value::as_array) {
                    for (index, item) in output.iter().enumerate() {
                        chunks.extend(self.terminal_item(index as u64, item)?);
                    }
                }
                let reason = finish_reason(response, !self.tools.is_empty())?;
                chunks.push(self.chunk(json!({}), json!(reason))?);
                if self.include_usage {
                    let usage = chat_usage(response.get("usage").ok_or(ChatResponseError)?)?;
                    let mut chunk = self.chunk(json!({}), Value::Null)?;
                    chunk["choices"] = json!([]);
                    chunk["usage"] = usage;
                    chunks.push(chunk);
                }
                self.terminal = true;
            }
            "response.failed" | "error" => {
                let error = data
                    .pointer("/response/error")
                    .or_else(|| data.get("error"))
                    .ok_or(ChatResponseError)?;
                if !error.is_object() {
                    return Err(ChatResponseError);
                }
                chunks.push(json!({"error": error}));
                self.terminal = true;
            }
            // 推理及生命周期事件没有标准 Chat delta，不把推理内容混入 assistant 正文
            _ => {}
        }
        Ok(chunks)
    }

    fn identity(&self) -> Result<&str, ChatResponseError> {
        self.id
            .as_deref()
            .filter(|id| !id.is_empty())
            .ok_or(ChatResponseError)
    }

    fn chunk(&self, delta: Value, finish: Value) -> Result<Value, ChatResponseError> {
        let mut chunk = json!({
            "id": self.identity()?, "object": "chat.completion.chunk", "created": self.created, "model": self.model,
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish, "logprobs": null}],
        });
        if self.include_usage {
            chunk["usage"] = Value::Null;
        }
        Ok(chunk)
    }

    fn content_chunk(&self, refusal: bool, text: &str) -> Result<Value, ChatResponseError> {
        self.chunk(
            if refusal {
                json!({"refusal": text})
            } else {
                json!({"content": text})
            },
            Value::Null,
        )
    }

    fn track_content(&mut self, key: (u64, u64)) -> Result<(), ChatResponseError> {
        if self.content.len() >= MAX_OUTPUT_ITEMS && !self.content.contains(&key) {
            return Err(ChatResponseError);
        }
        self.content.insert(key);
        Ok(())
    }

    fn tool_item(&mut self, item: &Value, done: bool) -> Result<Vec<Value>, ChatResponseError> {
        let id = item
            .get("id")
            .and_then(Value::as_str)
            .ok_or(ChatResponseError)?;
        let call_id = item
            .get("call_id")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .ok_or(ChatResponseError)?;
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .ok_or(ChatResponseError)?;
        let mut chunks = Vec::new();
        if !self.tools.contains_key(id) {
            if self.tools.len() >= MAX_OUTPUT_ITEMS
                || self.tools.values().any(|tool| tool.call_id == call_id)
            {
                return Err(ChatResponseError);
            }
            let index = self.tools.len();
            chunks.push(self.chunk(json!({"tool_calls": [{"index": index, "id": call_id, "type": "function", "function": {"name": name, "arguments": ""}}]}), Value::Null)?);
            self.tools.insert(
                id.to_owned(),
                ToolState {
                    index,
                    call_id: call_id.to_owned(),
                    name: name.to_owned(),
                    arguments_sent: false,
                },
            );
        }
        let tool = self.tools.get_mut(id).ok_or(ChatResponseError)?;
        if tool.call_id != call_id || tool.name != name {
            return Err(ChatResponseError);
        }
        if done && !tool.arguments_sent {
            let arguments = item
                .get("arguments")
                .and_then(Value::as_str)
                .ok_or(ChatResponseError)?;
            tool.arguments_sent = true;
            let index = tool.index;
            chunks.push(self.chunk(
                json!({"tool_calls": [{"index": index, "function": {"arguments": arguments}}]}),
                Value::Null,
            )?);
        }
        Ok(chunks)
    }

    fn terminal_item(&mut self, index: u64, item: &Value) -> Result<Vec<Value>, ChatResponseError> {
        match item.get("type").and_then(Value::as_str) {
            Some("function_call") => self.tool_item(item, true),
            Some("message") => {
                let mut chunks = Vec::new();
                for (part_index, part) in item
                    .get("content")
                    .and_then(Value::as_array)
                    .ok_or(ChatResponseError)?
                    .iter()
                    .enumerate()
                {
                    let key = (index, part_index as u64);
                    if self.content.contains(&key) {
                        continue;
                    }
                    let (refusal, text) = match part.get("type").and_then(Value::as_str) {
                        Some("output_text") => (false, part.get("text")),
                        Some("refusal") => (true, part.get("refusal")),
                        _ => return Err(ChatResponseError),
                    };
                    self.track_content(key)?;
                    chunks.push(self.content_chunk(
                        refusal,
                        text.and_then(Value::as_str).ok_or(ChatResponseError)?,
                    )?);
                }
                Ok(chunks)
            }
            Some("reasoning") => Ok(Vec::new()),
            _ => Err(ChatResponseError),
        }
    }
}

fn content_key(data: &Value) -> Result<(u64, u64), ChatResponseError> {
    Ok((
        data.get("output_index")
            .and_then(Value::as_u64)
            .ok_or(ChatResponseError)?,
        data.get("content_index")
            .and_then(Value::as_u64)
            .ok_or(ChatResponseError)?,
    ))
}
