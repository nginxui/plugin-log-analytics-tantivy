// Sorting and filtering of the index columns of the log list. The host loads
// the whole list at once and applies these in the browser.
import type { ColumnFilter, NginxLogRow } from '@nginxui/plugin-sdk'
import type { LogStatusItem } from '../api/types'
import { N_ } from '../gettext'

/** The part of the status store the column rules read. */
export interface StatusReader {
  statusFor: (path: string) => LogStatusItem | undefined
}

type SortValue = string | number | null | undefined

const STATUS_RANK: Record<string, number> = {
  indexed: 3,
  ready: 3,
  indexing: 2,
  not_indexed: 1,
}

/** Normalizes the states that mean the same thing for the user. */
export function normalizeStatus(status: string | undefined): string {
  if (status === 'ready')
    return 'indexed'
  return status || 'not_indexed'
}

export function statusRank(status: string | undefined): number {
  return STATUS_RANK[status ?? ''] ?? 0
}

/** Zero and missing values sort last, like the empty cells they render. */
function presentNumber(value: number | undefined): number | undefined {
  return value || undefined
}

export function timerangeOf(item: LogStatusItem | undefined): { start: number, end: number } | undefined {
  if (!item)
    return undefined

  const start = item.timerange?.start ?? item.timerange_start
  const end = item.timerange?.end ?? item.timerange_end
  if (!start || !end)
    return undefined

  return { start, end }
}

export function createSortValues(store: StatusReader): Record<'index_status' | 'last_indexed' | 'document_count', (row: NginxLogRow) => SortValue> {
  return {
    index_status: row => statusRank(store.statusFor(row.path)?.index_status),
    last_indexed: row => presentNumber(store.statusFor(row.path)?.last_indexed),
    document_count: row => presentNumber(store.statusFor(row.path)?.document_count),
  }
}

export function matchesStatus(store: StatusReader, row: NginxLogRow, value: string): boolean {
  return normalizeStatus(store.statusFor(row.path)?.index_status) === value
}

export function createStatusFilters(store: StatusReader): ColumnFilter[] {
  const options: Array<[string, string]> = [
    ['not_indexed', N_('Not Indexed')],
    ['queued', N_('Queued')],
    ['indexing', N_('Indexing')],
    ['indexed', N_('Indexed')],
    ['error', N_('Error')],
  ]

  // The labels are English source strings, the host translates them.
  return options.map(([value, label]) => ({
    label,
    value,
    match: row => matchesStatus(store, row, value),
  }))
}

/** Index columns show for the indexed log types. The host asks once per list. */
export function showsIndexColumns(context: { type?: unknown } | undefined): boolean {
  const type = context?.type
  return type === undefined || isIndexedType(type)
}

/** Access and error logs are indexed, other types are not. */
export function isIndexedType(type: unknown): boolean {
  return type === 'access' || type === 'error'
}
