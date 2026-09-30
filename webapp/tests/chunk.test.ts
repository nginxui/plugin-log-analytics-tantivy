import type { ChunkLoader } from '../src/chunk'
import { describe, expect, test } from 'bun:test'
import { createSSRApp, h } from 'vue'
import { renderToString } from 'vue/server-renderer'
import { createChunkView, resolveChunkComponent } from '../src/chunk'

const Viewer = {
  props: ['path'],
  render() {
    return h('div', { class: 'viewer' }, (this as unknown as { path: string }).path)
  },
}

describe('resolveChunkComponent', () => {
  test('returns the export the chunk handed over', async () => {
    const load = (async (name: string) => {
      expect(name).toBe('search')
      return { Structured: Viewer }
    }) as ChunkLoader

    expect(await resolveChunkComponent(load, 'search', 'Structured')).toBe(Viewer)
  })

  test('rejects when the chunk lacks the export', async () => {
    const load = (async () => ({ Other: Viewer })) as ChunkLoader
    await expect(resolveChunkComponent(load, 'search', 'Structured')).rejects.toThrow('does not export "Structured"')
  })

  test('passes a failing load on', async () => {
    const load = (async () => {
      throw new Error('network down')
    }) as ChunkLoader
    await expect(resolveChunkComponent(load, 'dashboard', 'Dashboard')).rejects.toThrow('network down')
  })
})

describe('createChunkView', () => {
  test('renders the loading state before the chunk arrived', async () => {
    const view = createChunkView({
      chunk: 'search',
      exportName: 'Structured',
      getLoader: () => (() => new Promise(() => {})) as ChunkLoader,
      warmUp: () => {},
    })

    const html = await renderToString(createSSRApp({ render: () => h(view, { path: '/a.log' }) }))
    expect(html).toContain('min-height: 30vh')
    expect(html).not.toContain('viewer')
  })

  test('has one component per chunk so the host keeps them apart', () => {
    const options = { exportName: 'X', getLoader: () => undefined, warmUp: () => {} }
    const first = createChunkView({ ...options, chunk: 'search' }) as { name?: string }
    const second = createChunkView({ ...options, chunk: 'dashboard' }) as { name?: string }
    expect(first.name).not.toBe(second.name)
  })
})
