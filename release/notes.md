# v3.30.1+lqy007700

## Chat 参数兼容

- 修复 Chat 客户端携带 `max_completion_tokens` 时在 `/v1/chat/completions` 入口被拒绝的问题，普通与流式请求均可接入
- 正整数值映射为 Responses `max_output_tokens`，`null` 沿用上游默认值；非法值返回 `400` 并指明原始 Chat 参数
- 保留 Provider 对上游参数的既有处理：Codex 路径会移除 `max_output_tokens`，接受该参数不表示能够限制生成 Token 或费用

## 接入与升级

- 相对 `v3.30.0+lqy007700` 没有新增数据库迁移、配置要求或插件协议变更
- 其他不支持参数的范围见 [API 参考](https://github.com/lqy007700/codex-proxy-rs/blob/v3.30.1+lqy007700/docs/api.md#chat-completions)，包括 `max_tokens`、`temperature`、`top_p`、`stop` 等；本版本仅补齐 `max_completion_tokens` 兼容
- 提供 Linux amd64 归档与容器镜像，Docker 镜像标签为 `3.30.1-lqy007700`
