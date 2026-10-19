# v3.30.2+lqy007700

## Chat 参数兼容

- 修复 New API 等客户端携带 `max_completion_tokens` 时在 `/v1/chat/completions` 入口被拒绝的问题，支持普通与流式请求
- 正整数值映射为 Responses `max_output_tokens`，`null` 沿用上游默认值；非法值返回 `400` 并指明原始 Chat 参数
- Codex 路径沿用既有 Provider 处理，会移除 `max_output_tokens`；接受该参数不表示能够限制生成 Token 或费用，完整参数范围见 [API 参考](https://github.com/lqy007700/codex-proxy-rs/blob/v3.30.2+lqy007700/docs/api.md#chat-completions)

## 请求统计修复

- 缩小统计维护的单批事务，让保留期删除修正能够在查询预算内持续提交，避免历史清理积压阻塞新请求统计

## 升级事项

- 相对 `v3.30.0+lqy007700` 没有新增数据库迁移、配置要求或插件协议变更
- 提供 Linux amd64 归档与容器镜像，Docker 镜像标签为 `3.30.2-lqy007700`
