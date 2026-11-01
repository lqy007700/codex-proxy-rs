# v3.37.2+lqy007700.v2

## 构建修复

- 修复管理 API 测试夹具中的未使用字段，避免后端 Clippy 在 `-D warnings` 下因 dead code 阻断 Release 构建
- 本版本不改变运行时逻辑、数据库迁移或业务配置
