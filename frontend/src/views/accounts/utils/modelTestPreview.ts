import type { AccountModelTestDetail } from '@/api'
import createDOMPurify from 'dompurify'

import { accountModelTestImageUrl } from '@/api'

export type AccountModelTestPreview
  = | { kind: 'image', source: string }
    | { kind: 'document', source: string }

const svgStart = /<svg[\s>]/i
const documentStart = /<!doctype\s+html[\s>]|<html[\s>]/i
const fenceStart = /^(`{3,}|~{3,})(?:html|svg|xml)?$/i
const policy = 'default-src \'none\'; script-src \'none\'; style-src \'unsafe-inline\'; img-src data:; base-uri \'none\'; form-action \'none\''
const purifier = createDOMPurify()
purifier.addHook('uponSanitizeAttribute', (_node, attribute) => {
  const { attrName, attrValue } = attribute
  if ((attrName === 'href' || attrName === 'xlink:href') && !attrValue.startsWith('#'))
    attribute.keepAttr = false
  if (attrName === 'attributename' && /href|src/i.test(attrValue))
    attribute.keepAttr = false
})

function extractMarkup(text: string) {
  if (!svgStart.test(text))
    return ''

  const lines = text.trim().split('\n')
  let opening = -1
  let delimiter = ''
  for (const [index, line] of lines.entries()) {
    const trimmed = line.trim()
    if (opening >= 0) {
      if (trimmed === delimiter) {
        const markup = lines.slice(opening + 1, index).join('\n').trim()
        if (svgStart.test(markup))
          return markup
        opening = -1
      }
    }
    else {
      const match = fenceStart.exec(trimmed)
      if (match?.[1]) {
        opening = index
        delimiter = match[1]
      }
    }
  }

  const source = text.trim()
  const start = source.search(documentStart)
  if (start >= 0) {
    const end = source.toLowerCase().lastIndexOf('</html>')
    return source.slice(start, end >= start
      ? end + '</html>'.length
      : undefined)
  }
  return /^(?:<\?xml[^>]*>\s*)?<svg[\s>]/i.test(source)
    ? source
    : ''
}

function animationDocument(text: string) {
  const markup = extractMarkup(text)
  if (!markup || !purifier.isSupported)
    return ''

  // 保留本地 SVG 引用和 CSS 控件，禁止导航、自刷新及嵌套文档
  const documentElement = purifier.sanitize(markup, {
    WHOLE_DOCUMENT: true,
    RETURN_DOM: true,
    USE_PROFILES: { html: true, svg: true, svgFilters: true },
    ADD_TAGS: ['animate', 'use'],
    ADD_ATTR: (name, tag) => ['from', 'to', 'calcmode'].includes(name) && tag.startsWith('animate'),
    FORBID_TAGS: [
      'a',
      'area',
      'base',
      'embed',
      'foreignobject',
      'form',
      'iframe',
      'link',
      'meta',
      'object',
      'script',
      'template',
    ],
    FORBID_ATTR: [
      'action',
      'autofocus',
      'background',
      'formaction',
      'ping',
      'poster',
      'src',
      'srcdoc',
      'srcset',
      'target',
    ],
  })
  if (!(documentElement instanceof Element))
    return ''

  const head = documentElement.querySelector('head')
  if (!head || !documentElement.querySelector('svg'))
    return ''

  const meta = document.createElement('meta')
  meta.httpEquiv = 'Content-Security-Policy'
  meta.content = policy
  head.prepend(meta)
  if (/^(?:<\?xml[^>]*>\s*)?<svg[\s>]/i.test(markup)) {
    const style = document.createElement('style')
    style.textContent = 'body{margin:0;display:grid;min-height:100vh;place-items:center}body>svg{width:100%;height:auto;max-height:100vh}'
    head.append(style)
  }
  return `<!doctype html>${documentElement.outerHTML}`
}

export function accountModelTestPreview(result: Pick<AccountModelTestDetail, 'id' | 'hasImage' | 'text'>): AccountModelTestPreview | undefined {
  if (result.hasImage) {
    return {
      kind: 'image',
      source: accountModelTestImageUrl(result.id),
    }
  }

  const source = result.text && animationDocument(result.text)
  if (source)
    return { kind: 'document', source }
}
