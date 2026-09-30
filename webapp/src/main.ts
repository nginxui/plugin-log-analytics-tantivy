// Entry point of the log analytics webapp bundle. Built as an IIFE by
// vite.config.ts and loaded by the host after login. It registers the slots,
// the settings panel and the translations, and stays small: the search and
// dashboard views are separate chunks fetched when a page first needs them.
import type { NginxUIPlugin, PluginRegistry } from '@nginxui/plugin-sdk'
import { registerPlugin } from '@nginxui/plugin-sdk'
import { PLUGIN_ID } from '../build.constants'
import { setAssetBase } from './assets'
import { translations } from './i18n'
import { resetRuntime, setRegistry } from './runtime'
import { registerSlots } from './slots'
import { disposeStatusStore } from './store/status'
import 'virtual:uno.css'

// The script is executing right now, so its address is the package's asset base.
setAssetBase((document.currentScript as HTMLScriptElement | null)?.src)

const plugin: NginxUIPlugin = {
  setup(registry: PluginRegistry) {
    setRegistry(registry)
    registerSlots(registry)

    for (const [locale, messages] of Object.entries(translations))
      registry.registerTranslations(locale, messages)
  },
  // The host calls this when the plugin is turned off while the page is open.
  // A later setup starts from a clean state.
  teardown() {
    disposeStatusStore()
    resetRuntime()
  },
}

registerPlugin(PLUGIN_ID, plugin)
