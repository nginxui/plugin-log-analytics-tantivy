// The plugin http client bypasses the host error interceptor, so the numbered
// errors of the backend are translated here. Codes and texts are the ones the
// host used before the log analytics code moved into the plugin.
import { $gettext } from './gettext'

const messages: Record<number, () => string> = {
  50011: () => $gettext('Log indexer not available'),
  50012: () => $gettext('Analytics service not available'),
  50013: () => $gettext('Log file does not exist'),
  50014: () => $gettext('Log path is not under whitelist'),
  50015: () => $gettext('Cannot access log file'),
  50016: () => $gettext('Background log service not available'),
  50017: () => $gettext('File path is required'),
  50018: () => $gettext('Failed to rebuild index'),
  50019: () => $gettext('Failed to rebuild file index'),
  50020: () => $gettext('Failed to delete file index'),
  50021: () => $gettext('Failed to delete all indexes'),
  50022: () => $gettext('Failed to get index status'),
  50023: () => $gettext('Failed to get persistence stats'),
  50024: () => $gettext('Log file is not a regular file'),
  50025: () => $gettext('Invalid websocket message type'),
  50026: () => $gettext('Modern searcher service not available'),
  50027: () => $gettext('Modern analytics service not available'),
  50028: () => $gettext('Modern indexer service not available'),
  50101: () => $gettext('Empty log line'),
  50102: () => $gettext('Log line exceeds maximum length'),
  50103: () => $gettext('Unsupported log format'),
  50104: () => $gettext('Invalid timestamp format'),
  50201: () => $gettext('Log parser is not initialized; call indexer.InitLogParser() before use'),
  60000: () => $gettext('Failed to download GeoLite2 database: {0}'),
  60001: () => $gettext('Failed to decompress GeoLite2 database: {0}'),
  60002: () => $gettext('GeoLite2 database not found at {0}'),
  60003: () => $gettext('Failed to get file size: {0}'),
  60004: () => $gettext('Failed to create file: {0}'),
  60005: () => $gettext('Failed to save downloaded file: {0}'),
  60006: () => $gettext('Failed to open file: {0}'),
  60007: () => $gettext('Failed to create xz reader: {0}'),
  60008: () => $gettext('Failed to write decompressed data: {0}'),
  60009: () => $gettext('Failed to read compressed data: {0}'),
  60010: () => $gettext('Decompression succeeded but failed to delete compressed file: {0}'),
}

export interface ApiErrorBody {
  scope?: string
  code?: number | string
  message?: string
  params?: string[]
}

/** Codes that mean the requested log file cannot be read at all. */
const PATH_ERROR_CODES = new Set([50013, 50014, 50015])

function substituteParams(message: string, params?: string[]): string {
  if (!params?.length)
    return message
  return params.reduce((result, param, index) => result.replaceAll(`{${index}}`, param), message)
}

/** Pulls the numbered error body out of an axios failure, if the backend sent one. */
export function readApiError(error: unknown): ApiErrorBody | undefined {
  if (!error || typeof error !== 'object')
    return undefined

  const data = (error as { response?: { data?: unknown } }).response?.data
  if (!data || typeof data !== 'object')
    return undefined

  return data as ApiErrorBody
}

export function errorCode(error: unknown): number | undefined {
  const code = Number(readApiError(error)?.code)
  return Number.isFinite(code) ? code : undefined
}

/** True for the errors that report an unreadable or missing log file. */
export function isPathError(error: unknown): boolean {
  const code = errorCode(error)
  return code !== undefined && PATH_ERROR_CODES.has(code)
}

/** Text to show for a failed request: the translated numbered error, or the fallback. */
export function errorMessage(error: unknown, fallback: string = $gettext('Server error')): string {
  const body = readApiError(error)
  const code = Number(body?.code)
  const translate = Number.isFinite(code) ? messages[code] : undefined
  if (translate)
    return substituteParams(translate(), body?.params)

  if (typeof body?.message === 'string' && body.message)
    return substituteParams($gettext(body.message), body.params)

  return fallback
}
