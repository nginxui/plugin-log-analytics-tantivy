// Test doubles for the status store: a manual clock and a scripted socket.
import type { SocketLike } from '../src/store/status'

interface Task {
  id: number
  at: number
  run: () => void
}

/** A clock the test advances by hand, so delays never cost real time. */
export function createTimers() {
  let now = 0
  let nextId = 1
  let tasks: Task[] = []

  return {
    setTimer(run: () => void, delay: number): number {
      const id = nextId++
      tasks.push({ id, at: now + delay, run })
      return id
    },
    clearTimer(handle: unknown) {
      tasks = tasks.filter(task => task.id !== handle)
    },
    /** Moves the clock and runs every task that became due, in order. */
    advance(ms: number) {
      const target = now + ms
      for (;;) {
        const due = tasks.filter(task => task.at <= target).sort((a, b) => a.at - b.at)[0]
        if (!due)
          break
        tasks = tasks.filter(task => task !== due)
        now = due.at
        due.run()
      }
      now = target
    },
    pending(): number {
      return tasks.length
    },
  }
}

export class FakeSocket implements SocketLike {
  onopen: ((event: unknown) => void) | null = null
  onmessage: ((event: { data: unknown }) => void) | null = null
  onclose: ((event: unknown) => void) | null = null
  onerror: ((event: unknown) => void) | null = null
  closed = false

  close() {
    this.closed = true
    this.onclose?.({})
  }

  open() {
    this.onopen?.({})
  }

  send(type: string, data: unknown) {
    this.onmessage?.({ data: JSON.stringify({ type, data }) })
  }

  drop() {
    this.onclose?.({})
  }
}

/** Lets pending promise callbacks run. */
export function flush(): Promise<void> {
  return new Promise(resolve => setTimeout(resolve, 0))
}
