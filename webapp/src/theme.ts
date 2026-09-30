import type { ComputedRef } from 'vue'
import { computed } from 'vue'
import { getLocale, getTheme } from './host'

/** Theme of the host, reactive. Charts and other canvas content read it. */
export function useHostTheme(): ComputedRef<'light' | 'dark'> {
  return computed(() => getTheme())
}

/** Locale code of the host, for example zh_CN, reactive. */
export function useHostLocale(): ComputedRef<string> {
  return computed(() => getLocale())
}
