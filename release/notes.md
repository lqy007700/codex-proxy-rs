# v3.55.2+lqy007700.v3

## 功能更新

- 新增 Antigravity（反重力）渠道，支持个人账号 Google OAuth 授权与版本 1 凭据文件导入导出、项目与模型目录同步、套餐与模型分组额度刷新，以及内置 Gemini / Claude / GPT-OSS 公开参考价目和本地计费
- 新增 Google GenAI 原生数据面入口（`/v1beta/models/{model}:generateContent`、`streamGenerateContent?alt=sse`、`countTokens`、`/v1beta/models` 模型目录与 `/v1beta:{method}` 后台指标），同时支持 Responses、Chat Completions 与 Anthropic Messages 协议转换路由到 Antigravity 模型
- 支持 Antigravity 模型分组与思考档位（`thinkingLevel`）解析、通用会话亲和（复用 `openaiAccountAffinity` 运行设置）、Desktop / CLI 客户端画像配置与自动版本跟踪，以及按部署时区汇总近一周本地用量
- 管理端在现有中文界面与 UI `v0.11.0` 上接入 Antigravity 图标、账号 OAuth / JSON 导入、客户端画像编辑器、请求明细画像展示及价目档位编辑

## 升级

- 无需新增数据库迁移，沿用现有账号、额度、模型目录、画像与运行设置表结构
- Antigravity 出站请求统一遵循账号所选代理，不读取环境代理；现有二开功能（账号级快速模式、降智测试、代理附加测试地址、xAI 自定义 API Key、告警通知、N/M 恢复与保底、账号级模型映射及可信调用方亲和）均保持兼容
