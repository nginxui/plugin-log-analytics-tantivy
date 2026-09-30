import type { App, Component } from 'vue'
import type { ChunkLoader } from '../src/chunk'
import { describe, expect, test } from 'bun:test'
import { createApp, h } from 'vue'
import { createChunkView } from '../src/chunk'
import { flush } from './helpers'

const Viewer: Component = {
  props: { path: { type: String, default: '' }, type: { type: String, default: '' } },
  render() {
    const self = this as unknown as { path: string, type: string }
    return h('div', { class: 'viewer' }, `${self.type}:${self.path}`)
  },
}

function mount(view: Component, props: Record<string, unknown>): { root: HTMLElement, app: App } {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp({ render: () => h(view, props) })
  app.mount(root)
  return { root, app }
}

describe('a chunk view mounted in a page', () => {
  test('shows a loading state, then the chunk component with the slot props', async () => {
    let resolveChunk: (exports: Record<string, unknown>) => void = () => {}
    const requested: string[] = []
    let warmed = 0

    const view = createChunkView({
      chunk: 'search',
      exportName: 'Structured',
      getLoader: () => (name => {
        requested.push(name)
        return new Promise(resolve => {
          resolveChunk = resolve as typeof resolveChunk
        })
      }) as ChunkLoader,
      warmUp: () => {
        warmed++
      },
    })

    const { root, app } = mount(view, { path: '/var/log/nginx/access.log', type: 'access' })
    await flush()

    expect(requested).toEqual(['search'])
    expect(warmed).toBe(1)
    expect(root.querySelector('.viewer')).toBeNull()
    expect(root.innerHTML).toContain('min-height: 30vh')

    resolveChunk({ Structured: Viewer })
    await flush()

    expect(root.querySelector('.viewer')?.textContent).toBe('access:/var/log/nginx/access.log')
    app.unmount()
  })

  test('shows an error instead of the view when the chunk fails to load', async () => {
    const view = createChunkView({
      chunk: 'dashboard',
      exportName: 'Dashboard',
      getLoader: () => (async () => {
        throw new Error('offline')
      }) as ChunkLoader,
      warmUp: () => {},
    })

    const originalError = console.error
    console.error = () => {}
    const { root, app } = mount(view, { path: '/a.log' })
    await flush()
    console.error = originalError

    expect(root.querySelector('.viewer')).toBeNull()
    expect(root.textContent).toContain('This view could not be loaded.')
    app.unmount()
  })

  test('shows an error on a host without chunk support', async () => {
    const view = createChunkView({
      chunk: 'dashboard',
      exportName: 'Dashboard',
      getLoader: () => undefined,
      warmUp: () => {},
    })

    const originalError = console.error
    console.error = () => {}
    const { root, app } = mount(view, { path: '/a.log' })
    await flush()
    console.error = originalError

    expect(root.textContent).toContain('This view could not be loaded.')
    app.unmount()
  })
})
