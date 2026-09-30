import { describe, expect, test } from 'bun:test'
import ja from '../src/i18n/ja_JP.json'
import zhCN from '../src/i18n/zh_CN.json'
import zhTW from '../src/i18n/zh_TW.json'
import { syntaxRows } from '../src/views/structured/components/search-syntax'

describe('search syntax help', () => {
  test('lists each example once, with a meaning', () => {
    const rows = syntaxRows()
    const examples = rows.flatMap(r => r.examples)
    expect(rows.length).toBeGreaterThan(5)
    expect(new Set(examples).size).toBe(examples.length)
    for (const row of rows) {
      expect(row.examples.length).toBeGreaterThan(0)
      expect(row.meaning.length).toBeGreaterThan(0)
    }
  })

  test('has translations for the required languages', () => {
    const catalogs: Record<string, Record<string, string>> = { zh_CN: zhCN, zh_TW: zhTW, ja_JP: ja }
    for (const row of syntaxRows()) {
      for (const [locale, catalog] of Object.entries(catalogs))
        expect(catalog[row.meaning], `${locale}: ${row.meaning}`).toBeTruthy()
    }
    for (const text of ['Search syntax', 'Separate words and filters with spaces. A line has to match all of them.']) {
      for (const [locale, catalog] of Object.entries(catalogs))
        expect(catalog[text], `${locale}: ${text}`).toBeTruthy()
    }
  })
})

describe('query warnings', () => {
  test('say what happened to each part in plain words', async () => {
    const { warningText } = await import('../src/views/structured/components/search-syntax')
    const reasons = ['unknown_field', 'invalid_value', 'unterminated_quote', 'too_many_terms', 'something_new']
    const texts = reasons.map(reason => warningText({ token: 'foo:bar', reason }))
    expect(new Set(texts).size).toBe(reasons.length)
    for (const text of texts.filter((_, i) => reasons[i] !== 'too_many_terms'))
      expect(text).toContain('foo:bar')
    for (const text of texts)
      expect(text).not.toMatch(/_|reason|token/)
  })
})
