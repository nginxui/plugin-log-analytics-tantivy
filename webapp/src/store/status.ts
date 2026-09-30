// One reactive cache of the index state of every log file. The list columns,
// the toolbar and the log views all read from it, so the status request and
// the /events websocket exist once no matter how many cells are on screen.
import type { ShallowRef } from 'vue'
import type {
  IndexCompleteEvent,
  IndexProgressEvent,
  IndexReadyEvent,
  LogStatusItem,
  LogStatusResponse,
  LogStatusSummary,
  ProcessingStatusEvent,
} from '../api/types'
import { onBeforeUnmount, shallowRef } from 'vue'
import * as api from '../api/client'
import { openPluginSocket } from '../host'
import { getRuntime } from '../runtime'

export interface IndexProgress {
  logPath: string
  progress: number
  stage: string
  status: string
  elapsedTime: number
  estimatedRemain: number
}

export type ConnectionState = 'closed' | 'connecting' | 'open'

/** The part of WebSocket the store uses, so tests can drive it with a stub. */
export interface SocketLike {
  onopen: ((event: unknown) => void) | null
  onmessage: ((event: { data: unknown }) => void) | null
  onclose: ((event: unknown) => void) | null
  onerror: ((event: unknown) => void) | null
  close: () => void
}

export interface StatusStoreDeps {
  fetchStatus: () => Promise<LogStatusResponse>
  openSocket: () => Promise<SocketLike>
  setTimer?: (callback: () => void, delay: number) => unknown
  clearTimer?: (handle: unknown) => void
}

/** Delay before the socket closes after the last consumer left. */
export const CLOSE_GRACE_MS = 5000
/** Status is refetched this often while indexing runs. */
export const POLL_INTERVAL_MS = 2000
const COMPLETE_KEEP_MS = 3000
const ERROR_KEEP_MS = 5000
const READY_DELAY_MS = 1000
const STOPPED_DELAY_MS = 500
const RECONNECT_MIN_MS = 1000
const RECONNECT_MAX_MS = 30000

export function createStatusStore(deps: StatusStoreDeps) {
  const setTimer = deps.setTimer ?? ((callback, delay) => setTimeout(callback, delay))
  const clearTimer = deps.clearTimer ?? (handle => clearTimeout(handle as ReturnType<typeof setTimeout>))

  const items: ShallowRef<Record<string, LogStatusItem>> = shallowRef({})
  const summary: ShallowRef<LogStatusSummary | undefined> = shallowRef(undefined)
  const progress: ShallowRef<Record<string, IndexProgress>> = shallowRef({})
  const isGlobalIndexing = shallowRef(false)
  const loaded = shallowRef(false)
  const connection = shallowRef<ConnectionState>('closed')

  const readyListeners = new Set<(event: IndexReadyEvent) => void>()
  let consumers = 0
  let socket: SocketLike | undefined
  let closeTimer: unknown
  let pollTimer: unknown
  let reconnectTimer: unknown
  let reconnectDelay = RECONNECT_MIN_MS
  let inflight: Promise<void> | undefined

  function statusFor(path: string): LogStatusItem | undefined {
    return items.value[path]
  }

  function progressFor(path: string): IndexProgress | undefined {
    return progress.value[path]
  }

  function isFileIndexing(path: string): boolean {
    return progress.value[path]?.status === 'running'
  }

  function setProgress(path: string, value: IndexProgress | undefined) {
    const next = { ...progress.value }
    if (value)
      next[path] = value
    else
      delete next[path]
    progress.value = next
  }

  /** True while the backend indexes, as the event stream or the file states tell. */
  function isIndexing(): boolean {
    return isGlobalIndexing.value || Object.values(items.value).some(item => item.index_status === 'indexing' || item.index_status === 'queued')
  }

  function schedulePoll() {
    if (pollTimer !== undefined || consumers === 0 || !isIndexing())
      return

    pollTimer = setTimer(() => {
      pollTimer = undefined
      void refresh()
    }, POLL_INTERVAL_MS)
  }

  function stopPoll() {
    if (pollTimer !== undefined) {
      clearTimer(pollTimer)
      pollTimer = undefined
    }
  }

  function refresh(): Promise<void> {
    if (inflight)
      return inflight

    inflight = deps.fetchStatus()
      .then(response => {
        const next: Record<string, LogStatusItem> = {}
        for (const item of response.items ?? []) {
          next[item.path] = item
          if (item.main_log_path && item.main_log_path !== item.path)
            next[item.main_log_path] ??= item
        }
        items.value = next
        summary.value = response.summary
        loaded.value = true
      })
      .catch(() => {
        // The columns keep showing the previous state, the next event retries.
      })
      .finally(() => {
        inflight = undefined
        schedulePoll()
      })

    return inflight
  }

  function handleProgress(data: IndexProgressEvent) {
    // A status update only reports a state change, it carries no progress.
    if (data.stage === 'status_update') {
      if (data.status !== 'indexing')
        setProgress(data.log_path, undefined)
      return
    }

    setProgress(data.log_path, {
      logPath: data.log_path,
      progress: data.progress,
      stage: data.stage,
      status: data.status,
      elapsedTime: data.elapsed_time,
      estimatedRemain: data.estimated_remain,
    })
  }

  function handleComplete(data: IndexCompleteEvent) {
    if (data.success) {
      setTimer(() => setProgress(data.log_path, undefined), COMPLETE_KEEP_MS)
    }
    else {
      setProgress(data.log_path, {
        logPath: data.log_path,
        progress: 0,
        stage: 'error',
        status: 'error',
        elapsedTime: data.duration,
        estimatedRemain: 0,
      })
      setTimer(() => setProgress(data.log_path, undefined), ERROR_KEEP_MS)
    }
    void refresh()
  }

  function handleReady(data: IndexReadyEvent) {
    setTimer(() => {
      void refresh()
      readyListeners.forEach(listener => listener(data))
    }, READY_DELAY_MS)
  }

  function handleProcessing(data: ProcessingStatusEvent) {
    const wasIndexing = isGlobalIndexing.value
    isGlobalIndexing.value = Boolean(data?.nginx_log_indexing)

    if (isGlobalIndexing.value && !wasIndexing) {
      void refresh()
    }
    else if (!isGlobalIndexing.value && wasIndexing) {
      progress.value = {}
      setTimer(() => void refresh(), STOPPED_DELAY_MS)
    }
  }

  /** Applies one decoded websocket message. */
  function handleEvent(type: string, data: unknown) {
    switch (type) {
      case 'nginx_log_index_progress':
        handleProgress(data as IndexProgressEvent)
        break
      case 'nginx_log_index_complete':
        handleComplete(data as IndexCompleteEvent)
        break
      case 'nginx_log_index_ready':
        handleReady(data as IndexReadyEvent)
        break
      case 'processing_status':
        handleProcessing(data as ProcessingStatusEvent)
        break
      default:
        break
    }
  }

  function handleMessage(raw: unknown) {
    if (typeof raw !== 'string')
      return
    try {
      const message = JSON.parse(raw) as { type?: string, data?: unknown }
      if (message.type)
        handleEvent(message.type, message.data)
    }
    catch {
      // A malformed frame is ignored, the next one carries the state again.
    }
  }

  function scheduleReconnect() {
    if (reconnectTimer !== undefined || consumers === 0)
      return

    reconnectTimer = setTimer(() => {
      reconnectTimer = undefined
      void connect()
    }, reconnectDelay)
    reconnectDelay = Math.min(reconnectDelay * 2, RECONNECT_MAX_MS)
  }

  async function connect(): Promise<void> {
    if (socket || connection.value === 'connecting' || consumers === 0)
      return

    connection.value = 'connecting'
    let opened: SocketLike
    try {
      opened = await deps.openSocket()
    }
    catch {
      connection.value = 'closed'
      scheduleReconnect()
      return
    }

    // Every consumer left while the socket was being created.
    if (consumers === 0) {
      opened.close()
      connection.value = 'closed'
      return
    }

    socket = opened
    opened.onopen = () => {
      connection.value = 'open'
      reconnectDelay = RECONNECT_MIN_MS
      // Events sent while the socket was down are gone, so start from a fresh state.
      void refresh()
    }
    opened.onmessage = event => handleMessage(event.data)
    opened.onerror = () => {
      // onclose follows and drives the reconnect.
    }
    opened.onclose = () => {
      if (socket === opened)
        socket = undefined
      connection.value = 'closed'
      scheduleReconnect()
    }
  }

  function disconnect() {
    if (reconnectTimer !== undefined) {
      clearTimer(reconnectTimer)
      reconnectTimer = undefined
    }
    stopPoll()
    const current = socket
    socket = undefined
    connection.value = 'closed'
    if (current) {
      current.onclose = null
      current.close()
    }
  }

  /** Marks the store as needed. Returns the function that releases it again. */
  function acquire(): () => void {
    consumers += 1
    if (closeTimer !== undefined) {
      clearTimer(closeTimer)
      closeTimer = undefined
    }
    if (consumers === 1 || !loaded.value)
      void refresh()
    void connect()

    let released = false
    return () => {
      if (released)
        return
      released = true
      consumers -= 1
      if (consumers === 0) {
        closeTimer = setTimer(() => {
          closeTimer = undefined
          if (consumers === 0)
            disconnect()
        }, CLOSE_GRACE_MS)
      }
    }
  }

  /** Closes the socket and stops every timer, whoever still holds the store. */
  function dispose() {
    if (closeTimer !== undefined) {
      clearTimer(closeTimer)
      closeTimer = undefined
    }
    disconnect()
  }

  /** Calls back when the index of a file became ready. Returns the unsubscribe function. */
  function onIndexReady(listener: (event: IndexReadyEvent) => void): () => void {
    readyListeners.add(listener)
    return () => readyListeners.delete(listener)
  }

  return {
    items,
    summary,
    progress,
    isGlobalIndexing,
    loaded,
    connection,
    statusFor,
    progressFor,
    isFileIndexing,
    isIndexing,
    refresh,
    acquire,
    dispose,
    onIndexReady,
    handleEvent,
    handleMessage,
  }
}

export type StatusStore = ReturnType<typeof createStatusStore>

/** The store shared by the entry bundle and the chunks. */
export function useStatusStore(): StatusStore {
  const runtime = getRuntime()
  runtime.status ??= createStatusStore({
    fetchStatus: api.getLogStatus,
    openSocket: () => openPluginSocket('/events') as Promise<SocketLike>,
  })
  return runtime.status as StatusStore
}

/** Disposes the shared store, the next use creates a fresh one. */
export function disposeStatusStore(): void {
  const runtime = getRuntime()
  const store = runtime.status as StatusStore | undefined
  runtime.status = undefined
  store?.dispose()
}

/**
 * Store access for a component: holds the store while mounted, which opens the
 * socket on first use and asks the backend to warm its shards.
 */
export function useStatus(): StatusStore {
  const store = useStatusStore()
  api.warm()
  const release = store.acquire()
  onBeforeUnmount(release)
  return store
}
