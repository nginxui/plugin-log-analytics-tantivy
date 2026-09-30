// Builds the entry bundle of the log analytics plugin: one IIFE plus
// style.css and manifest.webapp.json. The heavy views live in the chunks
// built by vite.search.config.ts and vite.dashboard.config.ts.
import type { Plugin } from 'vite'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { nginxUiPlugin } from '@nginxui/plugin-sdk/vite'
import vue from '@vitejs/plugin-vue'
import UnoCSS from 'unocss/vite'
import { defineConfig } from 'vite'
import { PLUGIN_ID } from './build.constants'

// Country name files of i18n-iso-countries, one per language the host offers.
// They ship as static files so a view only fetches the language it shows.
const COUNTRY_LANGUAGES = ['en', 'zh', 'fr', 'es', 'de', 'ru', 'vi', 'ko', 'tr', 'ar', 'uk', 'ja', 'pt']

function countryNames(): Plugin {
  return {
    name: 'log-analytics-country-names',
    apply: 'build',
    generateBundle() {
      for (const language of COUNTRY_LANGUAGES) {
        const file = fileURLToPath(new URL(`./node_modules/i18n-iso-countries/langs/${language}.json`, import.meta.url))
        this.emitFile({
          type: 'asset',
          fileName: `assets/countries/${language}.json`,
          source: JSON.stringify(JSON.parse(readFileSync(file, 'utf8'))),
        })
      }
    },
  }
}

export default defineConfig({
  plugins: [
    vue(),
    UnoCSS(),
    nginxUiPlugin({ id: PLUGIN_ID, chunks: ['search', 'dashboard'] }),
    countryNames(),
  ],
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },
  define: {
    'process.env.NODE_ENV': JSON.stringify('production'),
  },
})
