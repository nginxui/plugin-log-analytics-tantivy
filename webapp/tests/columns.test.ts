import type { NginxLogRow } from '@nginxui/plugin-sdk'
import type { LogStatusItem } from '../src/api/types'
import { describe, expect, test } from 'bun:test'
import {
  createSortValues,
  createStatusFilters,
  matchesStatus,
  normalizeStatus,
  showsIndexColumns,
  statusRank,
  timerangeOf,
} from '../src/list/columns'
import { isDashboardAvailable, logKindOf } from '../src/rules'

function row(path: string, type: 'access' | 'error' = 'access'): NginxLogRow {
  return { path, type, name: path, config_file: '' }
}

function reader(items: Record<string, LogStatusItem>) {
  return { statusFor: (path: string) => items[path] }
}

describe('index status rules', () => {
  test('ready counts as indexed and a missing status as not indexed', () => {
    expect(normalizeStatus('ready')).toBe('indexed')
    expect(normalizeStatus('queued')).toBe('queued')
    expect(normalizeStatus(undefined)).toBe('not_indexed')
    expect(normalizeStatus('')).toBe('not_indexed')
  })

  test('ranks indexed above indexing above not indexed, other states last', () => {
    expect(statusRank('indexed')).toBeGreaterThan(statusRank('indexing'))
    expect(statusRank('indexing')).toBeGreaterThan(statusRank('not_indexed'))
    expect(statusRank('not_indexed')).toBeGreaterThan(statusRank('error'))
    expect(statusRank('ready')).toBe(statusRank('indexed'))
    expect(statusRank(undefined)).toBe(0)
  })
})

describe('column sort values', () => {
  const store = reader({
    '/a': { path: '/a', index_status: 'indexed', last_indexed: 500, document_count: 10 },
    '/b': { path: '/b', index_status: 'indexing', last_indexed: 0, document_count: 0 },
  })
  const values = createSortValues(store)

  test('index status sorts by rank, unknown files as not indexed... rank zero', () => {
    expect(values.index_status(row('/a'))).toBe(3)
    expect(values.index_status(row('/b'))).toBe(2)
    expect(values.index_status(row('/unknown'))).toBe(0)
  })

  test('zero and missing numbers become undefined so they sort last', () => {
    expect(values.last_indexed(row('/a'))).toBe(500)
    expect(values.last_indexed(row('/b'))).toBeUndefined()
    expect(values.last_indexed(row('/unknown'))).toBeUndefined()
    expect(values.document_count(row('/a'))).toBe(10)
    expect(values.document_count(row('/b'))).toBeUndefined()
  })

  test('sorting a list by the values matches the order of the old server side sort', () => {
    const rows = [row('/unknown'), row('/b'), row('/a')]
    const sorted = [...rows].sort((left, right) => (values.index_status(right) as number) - (values.index_status(left) as number))
    expect(sorted.map(item => item.path)).toEqual(['/a', '/b', '/unknown'])
  })
})

describe('status filters', () => {
  const store = reader({
    '/indexed': { path: '/indexed', index_status: 'indexed' },
    '/ready': { path: '/ready', index_status: 'ready' },
    '/indexing': { path: '/indexing', index_status: 'indexing' },
    '/queued': { path: '/queued', index_status: 'queued' },
    '/error': { path: '/error', index_status: 'error' },
    '/none': { path: '/none' },
  })
  const filters = createStatusFilters(store)
  const paths = Object.keys({ '/indexed': 1, '/ready': 1, '/indexing': 1, '/queued': 1, '/error': 1, '/none': 1, '/missing': 1 })

  function matching(value: string): string[] {
    const filter = filters.find(item => item.value === value)!
    return paths.filter(path => filter.match(row(path)))
  }

  test('offers the same choices as before, labelled with English source strings', () => {
    expect(filters.map(item => [item.value, item.label])).toEqual([
      ['not_indexed', 'Not Indexed'],
      ['queued', 'Queued'],
      ['indexing', 'Indexing'],
      ['indexed', 'Indexed'],
      ['error', 'Error'],
    ])
  })

  test('each choice keeps the rows in that state', () => {
    expect(matching('indexed')).toEqual(['/indexed', '/ready'])
    expect(matching('indexing')).toEqual(['/indexing'])
    expect(matching('queued')).toEqual(['/queued'])
    expect(matching('error')).toEqual(['/error'])
    expect(matching('not_indexed')).toEqual(['/none', '/missing'])
  })

  test('matchesStatus reads the store for the row path', () => {
    expect(matchesStatus(store, row('/error'), 'error')).toBe(true)
    expect(matchesStatus(store, row('/error'), 'indexed')).toBe(false)
  })
})

describe('time range', () => {
  test('reads the nested range or the flat fields', () => {
    expect(timerangeOf({ path: '/a', timerange: { start: 1, end: 2 } })).toEqual({ start: 1, end: 2 })
    expect(timerangeOf({ path: '/a', timerange_start: 3, timerange_end: 4 })).toEqual({ start: 3, end: 4 })
  })

  test('an empty or missing range is undefined', () => {
    expect(timerangeOf(undefined)).toBeUndefined()
    expect(timerangeOf({ path: '/a', timerange: { start: 0, end: 0 } })).toBeUndefined()
    expect(timerangeOf({ path: '/a' })).toBeUndefined()
  })
})

describe('where the views and columns appear', () => {
  test('index columns show for access and error logs', () => {
    expect(showsIndexColumns({ type: 'access' })).toBe(true)
    expect(showsIndexColumns({ type: 'error' })).toBe(true)
    expect(showsIndexColumns({ type: 'site' })).toBe(false)
    expect(showsIndexColumns({})).toBe(true)
    expect(showsIndexColumns(undefined)).toBe(true)
  })

  test('the dashboard is offered for access logs only', () => {
    expect(isDashboardAvailable({ path: '/var/log/nginx/access.log', type: 'access' })).toBe(true)
    expect(isDashboardAvailable({ path: '/var/log/nginx/access.log', type: 'error' })).toBe(false)
  })

  test('a site log of unknown type is judged by its path', () => {
    expect(logKindOf({ path: '/var/log/nginx/site.access.log', type: 'site' })).toBe('access')
    expect(logKindOf({ path: '/var/log/nginx/error.log', type: 'site' })).toBe('error')
    expect(logKindOf({ path: '/var/log/nginx/site_error_log', type: 'site' })).toBe('error')
    expect(isDashboardAvailable({ path: '/var/log/nginx/error.log', type: 'site' })).toBe(false)
  })
})
