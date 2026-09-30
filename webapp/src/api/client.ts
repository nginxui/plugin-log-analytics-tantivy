// Calls to the plugin backend. Everything goes through registry.http, whose
// base address is the plugin's own http route.
import type {
  AdvancedSearchRequest,
  AdvancedSearchResponse,
  AnalyticsRequest,
  ChinaCityMapRequest,
  ChinaMapData,
  CityData,
  DashboardAnalytics,
  DashboardRequest,
  GeoLiteStatus,
  LogStatusResponse,
  PreflightResponse,
  WorldMapData,
} from './types'
import { getHttp } from '../host'
import { getRuntime } from '../runtime'

const WARM_INTERVAL_MS = 60_000

export async function getLogStatus(): Promise<LogStatusResponse> {
  const { data } = await getHttp().get<LogStatusResponse>('/logs/status')
  return { items: data?.items ?? [], summary: data?.summary }
}

/**
 * Asks the backend to open its search shards ahead of the first query. Fire
 * and forget, sent at most once per minute; a failure is not worth reporting.
 */
export function warm(now: number = Date.now()): void {
  const runtime = getRuntime()
  if (runtime.warmedAt !== undefined && now - runtime.warmedAt < WARM_INTERVAL_MS)
    return

  runtime.warmedAt = now
  getHttp().post('/warm').catch(() => {
    // The next mount tries again after the interval.
  })
}

export async function rebuildIndex(): Promise<{ message: string }> {
  const { data } = await getHttp().post<{ message: string }>('/index/rebuild')
  return data
}

export async function rebuildFileIndex(path: string): Promise<{ message: string }> {
  const { data } = await getHttp().post<{ message: string }>('/index/rebuild', { path })
  return data
}

export async function search(request: AdvancedSearchRequest): Promise<AdvancedSearchResponse> {
  const { data } = await getHttp().post<AdvancedSearchResponse>('/search', request)
  return data
}

/**
 * Encodes a filesystem path for a query string, so a web application firewall
 * in front of nginx-ui does not mistake a log path for path traversal. The
 * backend accepts both forms.
 */
export function encodePathParam(path: string): string {
  let binary = ''
  for (const byte of new TextEncoder().encode(path))
    binary += String.fromCharCode(byte)

  return `b64_${btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')}`
}

export async function getPreflight(logPath?: string): Promise<PreflightResponse> {
  const params = logPath ? { log_path: encodePathParam(logPath) } : {}
  const { data } = await getHttp().get<PreflightResponse>('/preflight', { params })
  return data
}

export async function getDashboardAnalytics(request: DashboardRequest): Promise<DashboardAnalytics> {
  const { data } = await getHttp().post<DashboardAnalytics>('/dashboard', request)
  return data
}

export async function getWorldMapData(request: AnalyticsRequest): Promise<{ data: WorldMapData[] }> {
  const { data } = await getHttp().post<{ data: WorldMapData[] }>('/geo/world', request)
  return data
}

export async function getChinaMapData(request: AnalyticsRequest): Promise<{ data: ChinaMapData[] }> {
  const { data } = await getHttp().post<{ data: ChinaMapData[] }>('/geo/china', request)
  return data
}

export async function getChinaCityMapData(request: ChinaCityMapRequest): Promise<{ data: CityData[], top_data?: CityData[], custom_mmdb_mode?: boolean }> {
  const { data } = await getHttp().post<{ data: CityData[], top_data?: CityData[], custom_mmdb_mode?: boolean }>('/geo/china/city', request)
  return data
}

export async function getGeoLiteStatus(): Promise<GeoLiteStatus> {
  const { data } = await getHttp().get<GeoLiteStatus>('/geolite/status')
  return data
}

/** Reads a China boundary file from the plugin, the caller falls back to a public mirror. */
export async function getGeoBoundary(filename: string): Promise<unknown> {
  const { data } = await getHttp().get<unknown>(`/geo/boundary/${encodeURIComponent(filename)}`)
  return data
}
