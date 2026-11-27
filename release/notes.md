# v3.57.1+lqy007700.v1

本版选择性移植上游 `v3.57.1`（`eece08a4a`）的 OpenAI WebSocket 连接复用与 Antigravity 独立搜索修复

## OpenAI

- WebSocket 连接按响应字段和流事件字段的排除设置复用，避免沿用上一请求的排除设置
- 预热保留握手返回的模型目录 ETag，在该连接上首次实际生成时交付

## Antigravity

- 原生非流式独立 Google Search 可使用分组模型名，省略 `thinkingConfig`，并沿用搜索选项和账号授权规则

## 升级

- 本版沿用数据库迁移 `0064`
