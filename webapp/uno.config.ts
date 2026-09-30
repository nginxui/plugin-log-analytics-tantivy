// Utility classes used by the plugin views. Every class carries the "la-"
// prefix and no reset is emitted, so the stylesheet cannot leak into the host.
import { defineConfig, presetUno } from 'unocss'

export default defineConfig({
  presets: [
    presetUno({
      prefix: 'la-',
      variablePrefix: 'la-',
      preflight: false,
      dark: 'class',
    }),
  ],
  content: {
    pipeline: {
      include: [/\.(vue|[jt]sx|ts)($|\?)/],
    },
  },
})
