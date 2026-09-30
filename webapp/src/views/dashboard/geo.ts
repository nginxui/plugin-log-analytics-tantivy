// Country names and locale rules of the geographic charts. The names of a
// language are separate files of the package, only the ones in use are fetched.
import type { ComputedRef } from 'vue'
import countries from 'i18n-iso-countries'
import { computed, ref, watchEffect } from 'vue'
import { fetchAssetJson } from '@/assets'
import { useHostLocale } from '@/theme'

export interface GeoData {
  code: string
  name?: string
  region?: string
  province?: string
  city?: string
  isp?: string
  value: number
  percent: number
}

// Host locale to the language code of the country name files. Simplified and
// traditional Chinese share one file.
const LANGUAGE_CODES: Record<string, string> = {
  en: 'en',
  zh_CN: 'zh',
  zh_TW: 'zh',
  fr_FR: 'fr',
  es: 'es',
  de_DE: 'de',
  ru_RU: 'ru',
  vi_VN: 'vi',
  ko_KR: 'ko',
  tr_TR: 'tr',
  ar: 'ar',
  uk_UA: 'uk',
  ja_JP: 'ja',
  pt_PT: 'pt',
}

export function languageCodeOf(locale: string): string {
  return LANGUAGE_CODES[locale] ?? 'en'
}

export function isChineseLocaleCode(locale: string): boolean {
  return locale === 'zh_CN' || locale === 'zh_TW'
}

// Bumped whenever a language finished loading, so names re-render.
const loadedVersion = ref(0)
const loading = new Map<string, Promise<void>>()

/** Loads and registers the country names of a language. Resolves when they are usable. */
export function ensureCountryNames(language: string): Promise<void> {
  let pending = loading.get(language)
  if (!pending) {
    pending = fetchAssetJson<Parameters<typeof countries.registerLocale>[0]>(`assets/countries/${language}.json`)
      .then(locale => {
        countries.registerLocale(locale)
        loadedVersion.value += 1
      })
      .catch(() => {
        // Names fall back to the country code, a later view retries.
        loading.delete(language)
      })
    loading.set(language, pending)
  }
  return pending
}

export function useGeoTranslation() {
  const hostLocale = useHostLocale()
  const locale: ComputedRef<string> = computed(() => languageCodeOf(hostLocale.value))
  const isChineseLocale = computed(() => isChineseLocaleCode(hostLocale.value))

  // The English names match the map outlines, the current language is for display.
  const ready = ref(false)
  watchEffect(() => {
    const language = locale.value
    void Promise.all([ensureCountryNames('en'), ensureCountryNames(language)]).then(() => {
      ready.value = true
    })
  })

  // Translate country code to localized name
  function translateCountry(countryCode: string): string {
    if (!countryCode)
      return ''

    // Handle special cases
    if (countryCode === 'UNKNOWN')
      return isChineseLocale.value ? '未知' : 'Unknown'

    void loadedVersion.value
    return countries.getName(countryCode, locale.value) || countryCode
  }

  // Format geographic display based on locale
  function formatGeoDisplay(data: GeoData): string {
    if (isChineseLocale.value) {
      // Chinese locales show the detailed geographic info
      const parts = [data.province, data.city].filter((part): part is string => Boolean(part))
      return parts.length > 0 ? parts.join(' ') : translateCountry(data.code)
    }

    // Other locales only show the country name
    return translateCountry(data.code)
  }

  return {
    locale,
    ready,
    isChineseLocale,
    translateCountry,
    formatGeoDisplay,
  }
}
