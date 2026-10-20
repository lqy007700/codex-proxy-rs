const clients: [RegExp, string][] = [
  [/\bcodex[ -]desktop\b/i, 'Codex Desktop'],
  [/\bcodex(?:_cli_rs|-cli)\//i, 'Codex CLI'],
  [/^pi(?:[\s/]|$)/i, 'Pi'],
  [/\bgrok[_-]cli\//i, 'Grok CLI'],
  [/\bclaude-(?:code|cli)\//i, 'Claude Code'],
  [/\bopenai-python\//i, 'OpenAI Python'],
  [/\b(?:openai-node\/|openai\/js\b)/i, 'OpenAI JS'],
  [/\b(?:Edg|EdgA|EdgiOS)\//i, 'Edge'],
  [/\bOPR\//i, 'Opera'],
  [/\b(?:Firefox|FxiOS)\//i, 'Firefox'],
  [/\b(?:Chrome|CriOS)\//i, 'Chrome'],
  [/\bVersion\/.*\bSafari\//i, 'Safari'],
  [/^curl\//i, 'curl'],
  [/^python-requests\//i, 'Python Requests'],
]

const systems: [RegExp, string][] = [
  [/\b(?:iPhone|iPad|iPod)\b/i, 'iOS'],
  [/\bAndroid\b/i, 'Android'],
  [/\b(?:Windows|win32|win64)\b/i, 'Windows'],
  [/\b(?:Mac OS|Macintosh|macOS|darwin)\b/i, 'macOS'],
  [/\bLinux\b/i, 'Linux'],
]

export function summarizeUserAgent(userAgent: string | null | undefined) {
  if (!userAgent?.trim())
    return null

  const client = clients.find(([pattern]) => pattern.test(userAgent))?.[1] ?? '未知客户端'
  const system = systems.find(([pattern]) => pattern.test(userAgent))?.[1]
  // 使用完整原文计算稳定代号，版本、系统等差异也参与区分；代号只用于展示
  let hash = 0x811C9DC5
  for (let index = 0; index < userAgent.length; index++) {
    hash = Math.imul(hash ^ userAgent.charCodeAt(index), 0x01000193)
  }

  return {
    label: system ? `${client} · ${system}` : client,
    code: (hash >>> 0).toString(16).padStart(8, '0').toUpperCase(),
  }
}
