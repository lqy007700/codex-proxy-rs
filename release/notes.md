# v3.55.2+lqy007700.v4

## 问题修复

- OpenAI Responses 出站请求（覆盖 Codex OAuth 与 API Key Responses 的 HTTP SSE 及 WebSocket 传输）自动省略输入历史中不符合类型前缀要求的可选内联条目 `id`（含 `message`、无密文 `reasoning` 以及 `function_call` / `custom_tool_call` 与对应输出），避免跨模型或经第三方中转切换后因 `invalid_id_prefix` 返回 400
- 保留合法类型前缀 `id`、`item_reference` 引用的条目 `id` 以及绑定非空密文的条目 `id`，不影响原生会话缓存命中与密文校验

## 升级

- 无需新增数据库迁移或调整配置，直接替换镜像或二进制升级即可
