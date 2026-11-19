# v3.53.0+lqy007700.v1

选择性整合 CPR 上游 v3.53.0（`e7f8b5a35`）

## 新增功能

- 请求明细支持组合筛选账号历史名称、邮箱、当前显示名称和错误来源
- 支持用 `openai.ws_pool.fast_path_budget_ms` 配置允许回退 HTTP 的请求等待上游 WebSocket 建连的时间，默认 800 毫秒，重启后生效
- 增加 GPT-6.2 Astra 和 Sol 内置价目，费用列区分 Fast 和 Ultrafast

## 问题修复

- 上游 WebSocket 按 Codex CLI 0.170.0 合同发送大消息。下游握手的轮次头只用于首次请求，当前帧的轮次状态优先
- 上游读取结束后归还账号调度槽位。原生 Responses 在响应提交前收到普通 WebSocket 参数错误时返回 HTTP 400 及上游错误对象
- 修复 Desktop 托管 TUI 的版本门禁和账号连接测试的过期回调。模型目录 ETag 一致时延后一小时复查

## 升级说明

- 本版沿用现有数据库迁移。请求明细的错误来源筛选需要 PostgreSQL 16 或更高版本，默认 Compose 使用 PostgreSQL 18
