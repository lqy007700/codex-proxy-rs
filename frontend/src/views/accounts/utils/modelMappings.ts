import { accountModelIdError } from './modelAccess'

export interface AccountModelMappingRow {
  requestedModel: string
  upstreamModel: string
}

export function accountModelMappingsError(rows: AccountModelMappingRow[]): string | undefined {
  if (rows.length > 256)
    return '最多配置 256 条模型映射'
  const sources = new Set<string>()
  for (const row of rows) {
    if (!row.requestedModel || !row.upstreamModel)
      return '请完整填写请求模型和上游模型'
    if (accountModelIdError(row.requestedModel) || accountModelIdError(row.upstreamModel))
      return '模型映射需要有效的精确模型 ID，最多 256 字节，不支持通配符'
    if (sources.has(row.requestedModel))
      return '同一个请求模型只能配置一条映射'
    sources.add(row.requestedModel)
  }
}
