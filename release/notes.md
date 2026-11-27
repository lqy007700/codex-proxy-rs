# v3.56.1+lqy007700.v1

本版从上游 `v3.56.1`（`75d0d0e0f8`）选择性移植修复，保留 Responses、Messages、Chat Completions 与 Antigravity 的现有适配。

## Antigravity

- Codex 模型选择器按当前密钥可用的模型分组和思考档位展示。Responses 显式联网搜索支持来源与正文引用。
- GenAI 接口支持 `x-goog-api-key`，按当前密钥可用的上游顺序展示模型。原生 SSE 等待时保持静默，首帧前失败保留 HTTP 错误。
- Responses WebSocket 预热在网关内完成，生成成功后的重连支持客户端携带完整上下文。
- 账号面板按模型组展示额度和用量，提供 Google AI 点数余额与使用设置。新请求记录保存发生时的套餐名称。

## Codex

- 无需 WebSocket 的 HTTP 压缩请求沿用 HTTP/SSE。

## 升级

- 数据库追加迁移 `0063`，为新记录保存套餐展示快照。升级前请保留一致性备份。
