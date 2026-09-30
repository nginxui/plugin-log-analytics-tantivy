// Bridges to the host gettext instance published on window.NginxUI.shared.
// Keys are English source strings, the translations come from
// registerTranslations in main.ts.
import { useShared } from '@nginxui/plugin-sdk'

interface GettextInstance {
  current: string
  $gettext: (msgid: string, params?: Record<string, string>) => string
}

type Params = Record<string, string | number>

function interpolate(msgid: string, params?: Params): string {
  if (!params)
    return msgid
  return msgid.replace(/%\{(\w+)\}/g, (match, name: string) => name in params ? String(params[name]) : match)
}

function instance(): GettextInstance | undefined {
  try {
    return useShared().gettext as GettextInstance
  }
  catch {
    return undefined
  }
}

export function $gettext(msgid: string, params?: Params): string {
  const gettext = instance()
  if (!gettext)
    return interpolate(msgid, params)

  const normalized = params
    ? Object.fromEntries(Object.entries(params).map(([key, value]) => [key, String(value)]))
    : undefined
  return gettext.$gettext(msgid, normalized)
}

/** Language code of the host, for example zh_CN. Empty when the host is not available. */
export function currentLanguage(): string {
  return instance()?.current ?? ''
}

/**
 * Marks a source string for extraction without translating it. Used for texts
 * the host translates itself, such as slot labels and column filter names.
 */
export function N_(msgid: string): string {
  return msgid
}
