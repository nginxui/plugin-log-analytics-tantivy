import type { LogStatusResponse } from '../src/api/types'
import { beforeEach, describe, expect, test } from 'bun:test'
import {
  CLOSE_GRACE_MS,
  createStatusStore,
  POLL_INTERVAL_MS,
} from '../src/store/status'
import { createTimers, FakeSocket, flush } from './helpers'

const idle: LogStatusResponse = {
  items: [
    { path: '/var/log/nginx/access.log', main_log_path: '/var/log/nginx/access.log', index_status: 'indexed', document_count: 12, last_indexed: 100 },
    { path: '/var/log/nginx/site.log', main_log_path: '/var/log/nginx/site.log', index_status: 'not_indexed' },
  ],
  summary: { total_files: 2, indexed_files: 1, indexing_files: 0, document_count: 12 },
}

function setup(responses: LogStatusResponse[] = [idle]) {
  const timers = createTimers()
  const sockets: FakeSocket[] = []
  let fetches = 0
  let opens = 0

  const store = createStatusStore({
    fetchStatus: async () => responses[Math.min(fetches++, responses.length - 1)],
    openSocket: async () => {
      opens++
      const socket = new FakeSocket()
      sockets.push(socket)
      return socket
    },
    setTimer: timers.setTimer,
    clearTimer: timers.clearTimer,
  })

  return { store, timers, sockets, fetchCount: () => fetches, openCount: () => opens }
}

describe('status store', () => {
  let env: ReturnType<typeof setup>

  beforeEach(() => {
    env = setup()
  })

  test('the first consumer fetches the status and opens one socket', async () => {
    const release1 = env.store.acquire()
    const release2 = env.store.acquire()
    await flush()

    expect(env.fetchCount()).toBe(1)
    expect(env.openCount()).toBe(1)
    expect(env.store.loaded.value).toBe(true)
    expect(env.store.statusFor('/var/log/nginx/access.log')?.document_count).toBe(12)
    expect(env.store.summary.value?.total_files).toBe(2)

    release1()
    release2()
  })

  test('items are also found by their main log path', async () => {
    const grouped = setup([{ items: [{ path: '/logs/access.log.1', main_log_path: '/logs/access.log', index_status: 'indexed' }] }])
    grouped.store.acquire()
    await flush()

    expect(grouped.store.statusFor('/logs/access.log')?.index_status).toBe('indexed')
    expect(grouped.store.statusFor('/logs/access.log.1')?.index_status).toBe('indexed')
  })

  test('progress events fill and clear the per file progress', async () => {
    env.store.acquire()
    await flush()
    const path = '/var/log/nginx/access.log'

    env.sockets[0].send('nginx_log_index_progress', {
      log_path: path,
      progress: 40,
      stage: 'indexing',
      status: 'running',
      elapsed_time: 2000,
      estimated_remain: 3000,
    })
    expect(env.store.progressFor(path)?.progress).toBe(40)
    expect(env.store.isFileIndexing(path)).toBe(true)

    // A status update that is not "indexing" only reports a state change.
    env.sockets[0].send('nginx_log_index_progress', { log_path: path, progress: 0, stage: 'status_update', status: 'queued', elapsed_time: 0, estimated_remain: 0 })
    expect(env.store.progressFor(path)).toBeUndefined()
  })

  test('a finished file keeps its bar for three seconds and refreshes the status', async () => {
    env.store.acquire()
    await flush()
    const path = '/var/log/nginx/access.log'
    env.sockets[0].send('nginx_log_index_progress', { log_path: path, progress: 100, stage: 'indexing', status: 'completed', elapsed_time: 1, estimated_remain: 0 })

    const before = env.fetchCount()
    env.sockets[0].send('nginx_log_index_complete', { log_path: path, success: true, duration: 10, total_lines: 5, indexed_size: 1 })
    await flush()

    expect(env.fetchCount()).toBe(before + 1)
    expect(env.store.progressFor(path)).toBeDefined()
    env.timers.advance(3000)
    expect(env.store.progressFor(path)).toBeUndefined()
  })

  test('a failed file shows an error state for five seconds', async () => {
    env.store.acquire()
    await flush()
    const path = '/var/log/nginx/access.log'

    env.sockets[0].send('nginx_log_index_complete', { log_path: path, success: false, duration: 10, total_lines: 0, indexed_size: 0, error: 'boom' })
    expect(env.store.progressFor(path)?.status).toBe('error')
    env.timers.advance(4999)
    expect(env.store.progressFor(path)).toBeDefined()
    env.timers.advance(1)
    expect(env.store.progressFor(path)).toBeUndefined()
  })

  test('processing status drives the global flag and clears progress when it stops', async () => {
    env.store.acquire()
    await flush()
    const path = '/var/log/nginx/access.log'

    env.sockets[0].send('processing_status', { nginx_log_indexing: true })
    expect(env.store.isGlobalIndexing.value).toBe(true)
    await flush()
    env.sockets[0].send('nginx_log_index_progress', { log_path: path, progress: 5, stage: 'indexing', status: 'running', elapsed_time: 1, estimated_remain: 1 })

    env.sockets[0].send('processing_status', { nginx_log_indexing: false })
    expect(env.store.isGlobalIndexing.value).toBe(false)
    expect(env.store.progressFor(path)).toBeUndefined()

    const before = env.fetchCount()
    env.timers.advance(500)
    await flush()
    expect(env.fetchCount()).toBe(before + 1)
  })

  test('an index ready event refreshes and notifies listeners after a second', async () => {
    env.store.acquire()
    await flush()
    const seen: string[] = []
    const stop = env.store.onIndexReady(event => seen.push(event.log_path))

    env.sockets[0].send('nginx_log_index_ready', { log_path: '/a.log', start_time: 1, end_time: 2, available: true, index_status: 'indexed' })
    expect(seen).toEqual([])
    env.timers.advance(1000)
    expect(seen).toEqual(['/a.log'])

    stop()
    env.sockets[0].send('nginx_log_index_ready', { log_path: '/b.log', start_time: 1, end_time: 2, available: true, index_status: 'indexed' })
    env.timers.advance(1000)
    expect(seen).toEqual(['/a.log'])
  })

  test('malformed frames and unknown events are ignored', async () => {
    env.store.acquire()
    await flush()

    env.sockets[0].onmessage?.({ data: 'not json' })
    env.sockets[0].onmessage?.({ data: new ArrayBuffer(2) })
    env.sockets[0].send('something_else', {})
    expect(env.store.isGlobalIndexing.value).toBe(false)
  })

  test('the socket closes a grace period after the last consumer left', async () => {
    const release = env.store.acquire()
    await flush()

    release()
    env.timers.advance(CLOSE_GRACE_MS - 1)
    expect(env.sockets[0].closed).toBe(false)
    env.timers.advance(1)
    expect(env.sockets[0].closed).toBe(true)
    expect(env.store.connection.value).toBe('closed')
  })

  test('a consumer that returns within the grace period keeps the socket', async () => {
    const release = env.store.acquire()
    await flush()
    release()
    env.timers.advance(1000)

    const again = env.store.acquire()
    env.timers.advance(CLOSE_GRACE_MS * 2)
    expect(env.sockets[0].closed).toBe(false)
    expect(env.openCount()).toBe(1)
    again()
  })

  test('dispose closes the socket at once and does not reconnect', async () => {
    const release = env.store.acquire()
    await flush()
    env.sockets[0].open()

    env.store.dispose()
    expect(env.sockets[0].closed).toBe(true)
    expect(env.store.connection.value).toBe('closed')

    // A component unmounted after the teardown releases its hold as usual.
    release()
    env.timers.advance(CLOSE_GRACE_MS * 10)
    expect(env.openCount()).toBe(1)
  })

  test('releasing twice only counts once', async () => {
    const first = env.store.acquire()
    const second = env.store.acquire()
    await flush()

    first()
    first()
    env.timers.advance(CLOSE_GRACE_MS)
    expect(env.sockets[0].closed).toBe(false)
    second()
  })

  test('a dropped socket reconnects with a growing delay while consumers remain', async () => {
    env.store.acquire()
    await flush()
    env.sockets[0].open()
    expect(env.store.connection.value).toBe('open')

    env.sockets[0].drop()
    expect(env.store.connection.value).toBe('closed')
    env.timers.advance(999)
    await flush()
    expect(env.openCount()).toBe(1)
    env.timers.advance(1)
    await flush()
    expect(env.openCount()).toBe(2)

    // The second socket never opens and drops too: the delay doubles.
    env.sockets[1].drop()
    env.timers.advance(1999)
    await flush()
    expect(env.openCount()).toBe(2)
    env.timers.advance(1)
    await flush()
    expect(env.openCount()).toBe(3)
  })

  test('opening the socket refetches the status to catch up on missed events', async () => {
    env.store.acquire()
    await flush()
    const before = env.fetchCount()

    env.sockets[0].open()
    await flush()
    expect(env.fetchCount()).toBe(before + 1)
  })

  test('the status is polled while files are indexing and stops when they are done', async () => {
    const busy = setup([
      { items: [{ path: '/a.log', index_status: 'indexing' }] },
      { items: [{ path: '/a.log', index_status: 'indexing' }] },
      { items: [{ path: '/a.log', index_status: 'indexed' }] },
    ])
    busy.store.acquire()
    await flush()
    expect(busy.fetchCount()).toBe(1)
    expect(busy.store.isIndexing()).toBe(true)

    busy.timers.advance(POLL_INTERVAL_MS)
    await flush()
    expect(busy.fetchCount()).toBe(2)

    busy.timers.advance(POLL_INTERVAL_MS)
    await flush()
    expect(busy.fetchCount()).toBe(3)
    expect(busy.store.isIndexing()).toBe(false)

    busy.timers.advance(POLL_INTERVAL_MS * 3)
    await flush()
    expect(busy.fetchCount()).toBe(3)
  })

  test('a failed status request keeps the previous state', async () => {
    let call = 0
    const store = createStatusStore({
      fetchStatus: async () => {
        if (call++ === 0)
          return idle
        throw new Error('offline')
      },
      openSocket: async () => new FakeSocket(),
    })

    store.acquire()
    await flush()
    await store.refresh()
    expect(store.statusFor('/var/log/nginx/access.log')?.index_status).toBe('indexed')
  })

  test('a socket that cannot be created is retried', async () => {
    const timers = createTimers()
    let attempts = 0
    const store = createStatusStore({
      fetchStatus: async () => idle,
      openSocket: async () => {
        attempts++
        if (attempts === 1)
          throw new Error('no token yet')
        return new FakeSocket()
      },
      setTimer: timers.setTimer,
      clearTimer: timers.clearTimer,
    })

    store.acquire()
    await flush()
    expect(attempts).toBe(1)
    expect(store.connection.value).toBe('closed')

    timers.advance(1000)
    await flush()
    expect(attempts).toBe(2)
  })
})
