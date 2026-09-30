// Minimal reader for the gettext .po catalogs of the host: msgid to msgstr,
// without context or plural entries and without fuzzy translations.

const ESCAPES: Record<string, string> = { 'n': '\n', 't': '\t', 'r': '\r', '"': '"', '\\': '\\' }

function unescapePo(value: string): string {
  return value.replace(/\\([ntr"\\])/g, (_match, key: string) => ESCAPES[key])
}

function readQuoted(line: string): string {
  const start = line.indexOf('"')
  const end = line.lastIndexOf('"')
  return start >= 0 && end > start ? unescapePo(line.slice(start + 1, end)) : ''
}

export function parsePo(source: string): Map<string, string> {
  const entries = new Map<string, string>()
  let msgid: string | undefined
  let msgstr = ''
  let field: 'msgid' | 'msgstr' | 'other' | undefined
  let fuzzy = false
  let skip = false

  function flush() {
    if (msgid !== undefined && msgid !== '' && !fuzzy && !skip && msgstr !== '')
      entries.set(msgid, msgstr)
    msgid = undefined
    msgstr = ''
    field = undefined
    fuzzy = false
    skip = false
  }

  for (const raw of source.split(/\r?\n/)) {
    const line = raw.trim()

    if (line === '') {
      flush()
    }
    else if (line.startsWith('#,')) {
      // A flag line opens a new entry once the previous one is complete.
      if (field === 'msgstr')
        flush()
      if (line.includes('fuzzy'))
        fuzzy = true
    }
    else if (line.startsWith('#')) {
      if (field === 'msgstr')
        flush()
    }
    else if (line.startsWith('msgctxt')) {
      if (field === 'msgstr')
        flush()
      skip = true
      field = 'other'
    }
    else if (line.startsWith('msgid_plural')) {
      skip = true
      field = 'other'
    }
    else if (line.startsWith('msgid')) {
      if (field === 'msgstr')
        flush()
      msgid = readQuoted(line)
      field = 'msgid'
    }
    else if (line.startsWith('msgstr[')) {
      skip = true
      field = 'other'
    }
    else if (line.startsWith('msgstr')) {
      msgstr = readQuoted(line)
      field = 'msgstr'
    }
    else if (line.startsWith('"')) {
      if (field === 'msgid')
        msgid = (msgid ?? '') + readQuoted(line)
      else if (field === 'msgstr')
        msgstr += readQuoted(line)
    }
  }
  flush()

  return entries
}

/** Source strings passed to $gettext( or N_( in a source file. */
export function extractMsgids(source: string): string[] {
  const found = new Set<string>()
  const call = /(?<![\w.])(?:\$gettext|N_)\(\s*(?:'((?:[^'\\]|\\.)*)'|"((?:[^"\\]|\\.)*)"|`((?:[^`\\$]|\\.)*)`)/g

  for (const match of source.matchAll(call)) {
    const text = match[1] ?? match[2] ?? match[3]
    found.add(text.replace(/\\(['"`\\])/g, '$1').replace(/\\n/g, '\n'))
  }

  return [...found]
}
