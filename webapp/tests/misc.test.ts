import type { PluginRegistry } from '@nginxui/plugin-sdk'
import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { extractMsgids, parsePo } from '../scripts/po'
import { encodePathParam, warm } from '../src/api/client'
import { errorCode, errorMessage, isPathError } from '../src/errors'
import { openPluginSocket } from '../src/host'
import { getRuntime, setRegistry } from '../src/runtime'
import { bytesToSize, formatDateTime, formatDuration, formatProgressTime, isErrorLogPath } from '../src/utils'

describe('plugin websockets', () => {
  const opened: string[] = []
  const realWebSocket = globalThis.WebSocket

  beforeEach(() => {
    opened.length = 0
    globalThis.WebSocket = class {
      constructor(url: string) {
        opened.push(url)
      }
    } as unknown as typeof WebSocket
  })

  afterEach(() => {
    globalThis.WebSocket = realWebSocket
  })

  test('connects to the address the host builds for the path', async () => {
    const asked: string[] = []
    setRegistry({
      wsUrl: (path: string) => {
        asked.push(path)
        return `wss://ui.example.com/api/plugins/x/http${path}?token=t`
      },
    } as unknown as PluginRegistry)

    await openPluginSocket('/events')
    expect(asked).toEqual(['/events'])
    expect(opened).toEqual(['wss://ui.example.com/api/plugins/x/http/events?token=t'])
  })

  test('fails on a host without wsUrl instead of building an address itself', async () => {
    setRegistry({} as unknown as PluginRegistry)

    await expect(openPluginSocket('/events')).rejects.toThrow('websockets')
    expect(opened).toEqual([])
  })
})

describe('warm', () => {
  const posts: string[] = []

  function install(post: () => Promise<unknown>) {
    setRegistry({
      http: {
        post: (url: string) => {
          posts.push(url)
          return post()
        },
      },
    } as unknown as PluginRegistry)
  }

  afterEach(() => {
    posts.length = 0
    getRuntime().warmedAt = undefined
  })

  test('is sent at most once per minute', () => {
    install(async () => ({}))
    warm(1_000)
    warm(30_000)
    warm(60_999)
    expect(posts).toEqual(['/warm'])
    warm(61_000)
    expect(posts).toEqual(['/warm', '/warm'])
  })

  test('ignores a failure', async () => {
    install(async () => {
      throw new Error('502')
    })
    expect(() => warm(5)).not.toThrow()
    await Promise.resolve()
  })
})

describe('api errors', () => {
  const failure = (data: unknown) => ({ response: { data } })

  test('translates a numbered error and fills its parameters', () => {
    expect(errorMessage(failure({ scope: 'nginx_log', code: 50018, message: 'x' }))).toBe('Failed to rebuild index')
    expect(errorMessage(failure({ code: '60000', params: ['timeout'] }))).toBe('Failed to download GeoLite2 database: timeout')
  })

  test('falls back to the message of the backend, then to the given text', () => {
    expect(errorMessage(failure({ code: 1, message: 'Something odd' }))).toBe('Something odd')
    expect(errorMessage(new Error('x'), 'Fallback')).toBe('Fallback')
  })

  test('recognizes the errors that mean the log file is unreadable', () => {
    expect(isPathError(failure({ code: 50013 }))).toBe(true)
    expect(isPathError(failure({ code: '50014' }))).toBe(true)
    expect(isPathError(failure({ code: 50015 }))).toBe(true)
    expect(isPathError(failure({ code: 50018 }))).toBe(false)
    expect(isPathError(undefined)).toBe(false)
    expect(errorCode(failure({}))).toBeUndefined()
  })
})

describe('formatting', () => {
  test('bytes use binary units', () => {
    expect(bytesToSize(0)).toBe('0 B')
    expect(bytesToSize(1536)).toBe('1.50 KiB')
    expect(bytesToSize(5 * 1024 ** 3)).toBe('5.00 GiB')
  })

  test('dates are local, with or without seconds', () => {
    const date = new Date(2026, 8, 30, 4, 5, 6)
    expect(formatDateTime(date)).toBe('2026-09-30 04:05')
    expect(formatDateTime(date, true)).toBe('2026-09-30 04:05:06')
  })

  test('durations and progress times read compactly', () => {
    expect(formatDuration(450)).toBe('450ms')
    expect(formatDuration(1500)).toBe('1.5s')
    expect(formatDuration(125_000)).toBe('2m 5s')
    expect(formatProgressTime(59_000)).toBe('59s')
    expect(formatProgressTime(3_725_000)).toBe('1h 2m 5s')
  })

  test('error logs are told apart by their path', () => {
    expect(isErrorLogPath('/var/log/nginx/error.log')).toBe(true)
    expect(isErrorLogPath('/var/log/nginx/site_error_log')).toBe(true)
    expect(isErrorLogPath('/var/log/nginx/access.log')).toBe(false)
  })
})

describe('translation extraction', () => {
  test('reads msgid and msgstr, joins continuation lines, skips fuzzy and plural entries', () => {
    const catalog = parsePo(`
msgid ""
msgstr "Language: zh_CN\\n"

msgid "Save"
msgstr "保存"

msgid "A long "
"sentence"
msgstr "长"
"句子"

#, fuzzy
msgid "Fuzzy"
msgstr "模糊"

msgid "Untranslated"
msgstr ""

msgid "One file"
msgid_plural "%d files"
msgstr[0] "一个"
msgstr[1] "多个"

msgctxt "menu"
msgid "Open"
msgstr "开"

msgid "Quote \\"me\\""
msgstr "引用 \\"我\\""
`)
    expect(catalog.get('Save')).toBe('保存')
    expect(catalog.get('A long sentence')).toBe('长句子')
    expect(catalog.has('Fuzzy')).toBe(false)
    expect(catalog.has('Untranslated')).toBe(false)
    expect(catalog.has('One file')).toBe(false)
    expect(catalog.has('Open')).toBe(false)
    expect(catalog.get('Quote "me"')).toBe('引用 "我"')
    expect(catalog.size).toBe(3)
  })

  test('finds strings passed to $gettext and N_ in every quoting style', () => {
    const found = extractMsgids(`
      $gettext('Single')
      $gettext("Double")
      $gettext(\`Template\`)
      :title="$gettext('In template')"
      $gettext('I\\'m escaped')
      N_('Marked')
      $gettext('With %{name}', { name })
      somethingelse$gettext('ignored')
      obj.$gettext('ignored too')
    `)
    expect(found.sort()).toEqual(['Double', 'I\'m escaped', 'In template', 'Marked', 'Single', 'Template', 'With %{name}'].sort())
  })
})

describe('path params', () => {
  test('encodes paths the way the backend decodes them', () => {
    expect(encodePathParam('/var/log/nginx/access.log')).toBe('b64_L3Zhci9sb2cvbmdpbngvYWNjZXNzLmxvZw')
    const encoded = encodePathParam('/日志/访问.log')
    expect(encoded.startsWith('b64_')).toBe(true)
    expect(encoded).toMatch(/^b64_[\w-]+$/)
    const base64 = encoded.slice(4).replace(/-/g, '+').replace(/_/g, '/')
    const bytes = Uint8Array.from(atob(base64), c => c.charCodeAt(0))
    expect(new TextDecoder().decode(bytes)).toBe('/日志/访问.log')
  })
})
