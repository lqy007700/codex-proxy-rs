# v3.38.1+lqy007700.v2

## 依赖安全修复

- 将 Axios 的间接依赖 `follow-redirects` 从 `1.16.1` 更新至 `1.16.2`，修复生产依赖安全审计报告的高危告警
- 对应公告为 [GHSA-mrwp-f3q8-3g9p / CVE-2026-116689](https://github.com/advisories/GHSA-mrwp-f3q8-3g9p)，涉及跨 JavaScript realm 的 `sensitiveHeaders` 数组校验与跨站重定向时自定义敏感请求头保护
- 仅定向更新依赖锁文件，Axios、UI、插件及其他依赖版本保持不变，不增加审计忽略项或降低检查级别

## 兼容与升级

- 保留 `v3.38.1+lqy007700.v1` 的 CPR Link v2、模型资格及全部二开能力，不修改后端请求恢复、保底账号、会话亲和或业务配置
- 本版本不新增数据库迁移，从 `v1` 升级无需额外配置
- 从更早版本升级仍需遵守 [v1 的迁移与备份要求](https://github.com/lqy007700/codex-proxy-rs/releases/tag/v3.38.1%2Blqy007700.v1)
