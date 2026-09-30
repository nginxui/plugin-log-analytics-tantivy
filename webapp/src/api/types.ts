// Request and response shapes of the plugin http API, identical to the ones
// the host handlers used before the log analytics code moved into the plugin.

export type IndexStatusKey = 'not_indexed' | 'queued' | 'indexing' | 'indexed' | 'ready' | 'error'

/** One log file as GET /logs/status reports it, keyed by the grouped main path. */
export interface LogStatusItem {
  path: string
  main_log_path?: string
  index_status?: IndexStatusKey | string
  last_modified?: number
  last_size?: number
  last_indexed?: number
  index_start_time?: number
  index_duration?: number
  is_compressed?: boolean
  has_timerange?: boolean
  timerange?: { start?: number, end?: number }
  timerange_start?: number
  timerange_end?: number
  document_count?: number
  error_message?: string
  error_time?: number
  retry_count?: number
  queue_position?: number
}

export interface LogStatusSummary {
  total_files?: number
  indexed_files?: number
  indexing_files?: number
  document_count?: number
}

export interface LogStatusResponse {
  items: LogStatusItem[]
  summary?: LogStatusSummary
}

export interface AnalyticsRequest {
  path: string
  start_time?: number
  end_time?: number
  limit?: number
}

export interface AccessLogEntry {
  timestamp: number
  ip: string
  method: string
  region_code: string
  province: string
  city: string
  c1?: string
  c2?: string
  c3?: string
  c4?: string
  ip_location_label?: string
  path: string
  protocol: string
  status: number
  bytes_sent: number
  referer: string
  user_agent: string
  browser: string
  browser_version: string
  os: string
  os_version: string
  device_type: string
  request_time?: number
  upstream_time?: number
  raw: string
}

export interface SearchFilters {
  query: string
  ip: string
  method: string
  status: string[]
  path: string
  user_agent: string
  referer: string
  browser: string[]
  os: string[]
  device: string[]
  /** Error log levels. */
  level: string[]
}

/** One error log entry, as the search returns it. */
export interface ErrorLogEntry {
  timestamp: number
  level: string
  pid: number
  connection: number
  message: string
  ip: string
  server: string
  request: string
  method: string
  path: string
  upstream: string
  host: string
  referer: string
  raw: string
}

export interface AdvancedSearchRequest {
  start_time?: number
  end_time?: number
  query?: string
  ip?: string
  method?: string
  status?: number[]
  path?: string
  user_agent?: string
  referer?: string
  browser?: string
  os?: string
  device?: string
  /** Error log levels, comma separated. */
  level?: string
  limit?: number
  offset?: number
  sort_by?: string
  sort_order?: string
  log_path?: string
}

export interface SummaryStats {
  uv: number
  pv: number
  total_traffic: number
  unique_pages: number
  avg_traffic_per_pv: number
  /** Traffic was extrapolated because the match set exceeded the scan budget. */
  traffic_approximate: boolean
}

export interface AdvancedSearchResponse {
  entries: (AccessLogEntry | ErrorLogEntry)[]
  total: number
  took: number
  query: string
  summary: SummaryStats
  /** Parts of the query that were taken as plain text. The pages do not use it. */
  query_warnings?: { token: string, reason: string }[]
}

export interface PreflightResponse {
  available: boolean
  index_status: string
  message?: string
  time_range?: {
    start: number
    end: number
  }
  file_info?: {
    exists: boolean
    readable: boolean
    size?: number
    last_modified?: number
  }
}

export interface DashboardRequest {
  log_path?: string
  /** YYYY-MM-DD */
  start_date?: string
  /** YYYY-MM-DD */
  end_date?: string
}

export interface HourlyStats {
  hour: number
  uv: number
  pv: number
  timestamp: number
}

export interface DailyStats {
  date: string
  uv: number
  pv: number
  timestamp: number
}

export interface URLStats {
  url: string
  visits: number
  percent: number
}

export interface BrowserStats {
  browser: string
  count: number
  percent: number
}

export interface OSStats {
  os: string
  count: number
  percent: number
}

export interface DeviceStats {
  device: string
  count: number
  percent: number
}

export interface DashboardSummary {
  total_uv: number
  total_pv: number
  /** Total bytes sent over the queried range. */
  total_traffic: number
  avg_daily_uv: number
  avg_daily_pv: number
  peak_hour: number
  peak_hour_traffic: number
  /** Requests per second across the queried range. */
  avg_qps: number
  /** Busiest minute of the range, expressed per second. */
  peak_qps: number
}

export interface DashboardAnalytics {
  hourly_stats: HourlyStats[]
  daily_stats: DailyStats[]
  top_urls: URLStats[]
  browsers: BrowserStats[]
  operating_systems: OSStats[]
  devices: DeviceStats[]
  summary: DashboardSummary
}

export interface WorldMapData {
  code: string
  value: number
  percent: number
  region?: string
  province?: string
  city?: string
  isp?: string
}

export interface CityData {
  name: string
  value: number
  percent: number
}

export interface ChinaMapData {
  name: string
  value: number
  percent: number
  cities?: CityData[]
}

export interface ChinaCityMapRequest extends AnalyticsRequest {
  province: string
}

export interface GeoLiteStatus {
  exists: boolean
  path: string
  size: number
  last_modified: string
}

/** Payloads of the /events websocket messages. */
export interface IndexProgressEvent {
  log_path: string
  progress: number
  stage: string
  status: string
  elapsed_time: number
  estimated_remain: number
}

export interface IndexCompleteEvent {
  log_path: string
  success: boolean
  duration: number
  total_lines: number
  indexed_size: number
  error?: string
}

export interface IndexReadyEvent {
  log_path: string
  start_time: number
  end_time: number
  available: boolean
  index_status: string
}

export interface ProcessingStatusEvent {
  nginx_log_indexing: boolean
}
