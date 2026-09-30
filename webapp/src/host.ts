// Access to the host through the registry captured in main.ts: the plugin
// http client, the theme and the address of plugin websockets.
import type { PluginHttpClient } from '@nginxui/plugin-sdk'
import { requireRegistry } from './runtime'

export function getHttp(): PluginHttpClient {
  return requireRegistry().http
}

export function getTheme(): 'light' | 'dark' {
  return requireRegistry().host.theme
}

export function getLocale(): string {
  return requireRegistry().host.locale
}

/**
 * Opens a websocket to a path of the plugin backend. The host builds the
 * address, because only it knows how a browser websocket authenticates.
 */
export async function openPluginSocket(path: string): Promise<WebSocket> {
  const { wsUrl } = requireRegistry()
  if (!wsUrl)
    throw new Error('[log-analytics] this host cannot open plugin websockets')

  return new WebSocket(wsUrl(path))
}
