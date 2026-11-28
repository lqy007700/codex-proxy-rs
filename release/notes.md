# v3.57.2+lqy007700.v1

本版选择性移植上游 `v3.57.2` 的 Antigravity GenAI 模型目录和 OpenAI 请求头透传改进

## Antigravity

- GenAI 模型目录按上游顺序展示当前 Key 可调用的模型，包含已声明的生图模型，并保留可用的上游描述与限制

## OpenAI

- Responses HTTP 和 Live 请求按业务头规则透传 `x-codex-turn-id`，保留同名头的多个值
