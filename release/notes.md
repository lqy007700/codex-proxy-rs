# v3.41.2+lqy007700.v6

## Key 安装身份

- 管理端新建 API Key 默认启用 `installationSeed`，按 Key 与账号稳定派生独立安装身份
- Key 编辑页新增 Seed 开关，可在 Seed 模式与旧 `installationId`／账号默认身份之间切换
- 保留现有亲和性、调度、账号模型映射、保底账号和 New API 传参判断逻辑

## 兼容与验证

- 继续保留旧 Key 的 `installationId` 双模式，不自动改变已有 Key 的安装身份
- 本次无新增数据库迁移
