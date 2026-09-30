// Loading of the on-demand chunks and the slot components that wait for them.
import type { Component } from 'vue'
import { Alert, Spin } from 'antdv-next'
import { defineComponent, h, onMounted, shallowRef } from 'vue'
import { warm } from './api/client'
import { $gettext } from './gettext'

export type ChunkLoader = <T = Record<string, unknown>>(name: string) => Promise<T>

/** Resolves the component a chunk exports under `exportName`. */
export async function resolveChunkComponent(load: ChunkLoader, chunk: string, exportName: string): Promise<Component> {
  const exports = await load<Record<string, unknown>>(chunk)
  const component = exports?.[exportName]
  if (!component)
    throw new Error(`chunk "${chunk}" does not export "${exportName}"`)
  return component as Component
}

export interface ChunkViewOptions {
  chunk: string
  exportName: string
  /** Returns registry.loadChunk, looked up when the view mounts. */
  getLoader: () => ChunkLoader | undefined
  /** Asks the backend to open its shards while the chunk downloads. */
  warmUp?: () => void
}

/**
 * A slot component that loads its chunk when it mounts and renders it with
 * the slot props once it arrived. Until then it shows a loading state.
 */
export function createChunkView(options: ChunkViewOptions): Component {
  const { chunk, exportName, getLoader, warmUp = warm } = options

  return defineComponent({
    name: `LogAnalyticsChunkView_${chunk}`,
    inheritAttrs: false,
    setup(_props, { attrs }) {
      const loaded = shallowRef<Component | undefined>()
      const failed = shallowRef(false)

      async function load() {
        failed.value = false
        try {
          const loader = getLoader()
          if (!loader)
            throw new Error('the host does not support chunks')
          loaded.value = await resolveChunkComponent(loader, chunk, exportName)
        }
        catch (error) {
          console.error(`[log-analytics] could not load chunk ${chunk}`, error)
          failed.value = true
        }
      }

      onMounted(() => {
        warmUp()
        void load()
      })

      return () => {
        if (loaded.value)
          return h(loaded.value, attrs)

        if (failed.value) {
          return h(Alert, {
            type: 'error',
            showIcon: true,
            title: $gettext('This view could not be loaded.'),
          })
        }

        return h('div', {
          style: 'display: flex; align-items: center; justify-content: center; min-height: 30vh',
        }, [h(Spin)])
      }
    },
  })
}
