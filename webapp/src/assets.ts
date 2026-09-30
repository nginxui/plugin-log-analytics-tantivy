// Static files of the plugin package (map outlines, country names). They are
// not part of any script: the host serves them from the package directory next
// to the bundle, and they are fetched when a view first needs them.
import { PLUGIN_ID } from '../build.constants'
import { getRuntime } from './runtime'

interface AssetState {
  base?: string
  cache: Map<string, Promise<unknown>>
}

const ASSET_KEY = Symbol.for(`${PLUGIN_ID}.assets`)

function state(): AssetState {
  const holder = getRuntime() as unknown as Record<symbol, AssetState | undefined>
  holder[ASSET_KEY] ??= { cache: new Map() }
  return holder[ASSET_KEY]
}

/** Remembers the directory the entry script was loaded from, for example .../webapp/dist/. */
export function setAssetBase(scriptUrl: string | undefined): void {
  if (!scriptUrl)
    return
  state().base = new URL('./', scriptUrl).toString()
}

export function assetUrl(path: string): string {
  const base = state().base ?? new URL(`plugins/${encodeURIComponent(PLUGIN_ID)}/webapp/dist/`, document.baseURI).toString()
  return new URL(path, base).toString()
}

/** Fetches a JSON file of the package once, later calls share the result. */
export function fetchAssetJson<T>(path: string): Promise<T> {
  const cache = state().cache
  let pending = cache.get(path) as Promise<T> | undefined
  if (!pending) {
    pending = fetch(assetUrl(path)).then(response => {
      if (!response.ok)
        throw new Error(`asset ${path} failed: ${response.status}`)
      return response.json() as Promise<T>
    })
    // A failed load is not kept, so the next view can try again.
    pending.catch(() => cache.delete(path))
    cache.set(path, pending)
  }
  return pending
}
