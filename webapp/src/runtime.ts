// State shared by the entry bundle and the on-demand chunks. Each output file
// carries its own copy of the modules it imports, so anything that has to be a
// singleton lives on a global under a registered symbol.
import type { PluginRegistry } from '@nginxui/plugin-sdk'
import { PLUGIN_ID } from '../build.constants'

export interface Runtime {
  registry?: PluginRegistry
  /** Status store, created on first use by store/status.ts. */
  status?: unknown
  /** Time of the last warm request in milliseconds. */
  warmedAt?: number
}

const RUNTIME_KEY = Symbol.for(`${PLUGIN_ID}.runtime`)

export function getRuntime(): Runtime {
  const holder = globalThis as unknown as Record<symbol, Runtime | undefined>
  holder[RUNTIME_KEY] ??= {}
  return holder[RUNTIME_KEY]
}

export function setRegistry(registry: PluginRegistry): void {
  getRuntime().registry = registry
}

/** Forgets the registry and the warm time. The status store is disposed by its own module. */
export function resetRuntime(): void {
  const runtime = getRuntime()
  runtime.registry = undefined
  runtime.warmedAt = undefined
}

export function requireRegistry(): PluginRegistry {
  const registry = getRuntime().registry
  if (!registry)
    throw new Error('[log-analytics] registry is not ready yet')
  return registry
}
