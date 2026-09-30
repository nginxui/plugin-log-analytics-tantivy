import { fileURLToPath } from 'node:url'
// Builds the "dashboard" chunk, loaded on demand through registry.loadChunk.
// Run it after the entry build: it keeps the files already in dist/.
import { nginxUiChunkPlugin } from '@nginxui/plugin-sdk/vite'
import vue from '@vitejs/plugin-vue'
import UnoCSS from 'unocss/vite'
import { defineConfig } from 'vite'
import { PLUGIN_ID } from './build.constants'

export default defineConfig({
  plugins: [
    vue(),
    UnoCSS(),
    nginxUiChunkPlugin({ id: PLUGIN_ID, name: 'dashboard', entry: 'src/dashboard.ts' }),
  ],
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },
  define: {
    'process.env.NODE_ENV': JSON.stringify('production'),
  },
  build: {
    copyPublicDir: false,
  },
})
