// Small helpers the entry bundle needs without pulling in a date library.

const SIZE_UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB', 'EiB', 'ZiB', 'YiB']

export function bytesToSize(bytes: number): string {
  if (!bytes)
    return '0 B'

  const unit = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), SIZE_UNITS.length - 1)
  return `${(bytes / 1024 ** unit).toFixed(2)} ${SIZE_UNITS[unit]}`
}

function pad(value: number, length = 2): string {
  return String(value).padStart(length, '0')
}

/** Local time, YYYY-MM-DD HH:mm or YYYY-MM-DD HH:mm:ss. */
export function formatDateTime(date: Date, withSeconds = false): string {
  const day = `${pad(date.getFullYear(), 4)}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`
  const time = `${pad(date.getHours())}:${pad(date.getMinutes())}`
  return withSeconds ? `${day} ${time}:${pad(date.getSeconds())}` : `${day} ${time}`
}

export function formatUnix(seconds: number, withSeconds = false): string {
  return formatDateTime(new Date(seconds * 1000), withSeconds)
}

/** Text form of an indexing duration given in milliseconds. */
export function formatDuration(milliseconds: number): string {
  if (milliseconds < 1000)
    return `${milliseconds}ms`
  if (milliseconds < 60000)
    return `${(milliseconds / 1000).toFixed(1)}s`

  const minutes = Math.floor(milliseconds / 60000)
  const seconds = Math.floor((milliseconds % 60000) / 1000)
  return `${minutes}m ${seconds}s`
}

/** Elapsed or remaining time in the compact form of the progress bar. */
export function formatProgressTime(milliseconds: number): string {
  const seconds = Math.floor(milliseconds / 1000)
  const minutes = Math.floor(seconds / 60)
  const hours = Math.floor(minutes / 60)

  if (hours > 0)
    return `${hours}h ${minutes % 60}m ${seconds % 60}s`
  if (minutes > 0)
    return `${minutes}m ${seconds % 60}s`
  return `${seconds}s`
}

export function isErrorLogPath(path: string): boolean {
  return path.includes('error.log') || path.includes('error_log')
}
