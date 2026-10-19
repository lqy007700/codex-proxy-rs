# v3.30.0+lqy007700

## Chat Completions 兼容接口

- 新增 `POST /v1/chat/completions`，支持普通聊天、SSE 流式响应、函数工具调用与结果回填、并行工具调用及图片输入
- 支持 `system`、`developer`、`user`、`assistant`、`tool` 消息，JSON Schema 输出格式、`reasoning_effort` 和流式用量块；多轮对话由客户端提交完整消息历史
- 复用 Responses 的鉴权、模型映射、账号调度、重试、并发、额度与计费，请求记录保留 Chat 入口，现有 Responses 接口继续可用
- 流式工具保持稳定索引并返回调用 ID，避免重复输出正文和工具参数；等待首帧与交付期间发送心跳，流内失败返回错误，不伪造成功结束状态

## 发行可靠性

- 修复单平台容器镜像发布，支持当前 Linux amd64 发行清单
- 恢复发布时固定发布工具来源，继续校验原发行的版本、提交和构建产物身份

## 接入与升级

- Chat 兼容接口不支持 `temperature`、`top_p`、`max_tokens`、`max_completion_tokens`、`stop`、`logprobs`、多候选、旧版 `functions/function_call`、消息 `name` 与音频；不支持的参数返回明确的 `400` 和 `param`，接入客户端须关闭这些参数
- 工具仅支持函数类型，`n` 仅支持 `1`，`store` 仅支持 `false`；图片与结构化输出的实际能力取决于上游模型，完整字段范围见 [API 参考](https://github.com/lqy007700/codex-proxy-rs/blob/v3.30.0+lqy007700/docs/api.md#chat-completions)
- 相对 `v3.29.3+lqy007700` 没有新增数据库迁移、配置要求或插件协议变更；继续提供 Linux amd64 归档与容器镜像，Docker 镜像标签为 `3.30.0-lqy007700`
