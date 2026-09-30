// Runs the built bundles against a stand-in for the host runtime: real vue and
// antdv-next behind window.NginxUI.shared, a scripted registry. It checks what
// unit tests cannot: that the IIFEs find their externals, register what the
// contract lists and render. It needs `bun run build` first and skips without it.
import type { Component } from 'vue'
import { existsSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import * as antdvIcons from '@antdv-next/icons'
import * as antdvNext from 'antdv-next'
import { afterAll, beforeAll, describe, expect, test } from 'bun:test'
import * as vue from 'vue'
import * as vueRouter from 'vue-router'
import { PLUGIN_ID } from '../build.constants'
import { useStatusStore } from '../src/store/status'
import { flush } from './helpers'

const dist = join(import.meta.dir, '../dist')
const built = existsSync(join(dist, 'main.js')) && existsSync(join(dist, 'chunks/search.js'))
const suite = built ? describe : describe.skip

interface Slot {
  name: string
  component: Component
  options: Record<string, unknown>
}

const slots: Slot[] = []
const translations: Record<string, Record<string, string>> = {}
const requests: string[] = []
const chunkExports: Record<string, Record<string, unknown>> = {}
let plugin: { setup: (registry: unknown) => void } | undefined
let settingsPanel: Component | undefined
const currentLocale = 'zh_CN'
let loadChunk: (name: string) => Promise<Record<string, unknown>>

class FakeWebSocket {
  static instances: FakeWebSocket[] = []
  onopen: (() => void) | null = null
  onmessage: ((event: { data: string }) => void) | null = null
  onclose: (() => void) | null = null
  onerror: (() => void) | null = null
  constructor(public url: string) {
    FakeWebSocket.instances.push(this)
  }

  close() {}
  send() {}
}

const statusBody = {
  items: [
    { path: '/var/log/nginx/access.log', index_status: 'indexed', document_count: 1234, last_indexed: 1_790_000_000, has_timerange: true, timerange: { start: 1_789_000_000, end: 1_790_000_000 } },
    { path: '/var/log/nginx/other.log', index_status: 'queued', queue_position: 2 },
  ],
  summary: { total_files: 2, indexed_files: 1, indexing_files: 0, document_count: 1234 },
}

const http = {
  async get(url: string) {
    requests.push(`GET ${url}`)
    if (url === '/logs/status')
      return { data: statusBody }
    if (url === '/preflight')
      return { data: { available: false, index_status: 'indexing' } }
    return { data: {} }
  },
  async post(url: string) {
    requests.push(`POST ${url}`)
    return { data: {} }
  },
}

function gettextInstance() {
  return {
    get current() {
      return currentLocale
    },
    $gettext(msgid: string, params?: Record<string, string>) {
      const text = translations[currentLocale]?.[msgid] ?? msgid
      return params ? text.replace(/%\{(\w+)\}/g, (_m, key: string) => params[key] ?? '') : text
    },
  }
}

function evaluate(file: string) {
  // eslint-disable-next-line no-eval
  ;(0, eval)(readFileSync(join(dist, file), 'utf8'))
}

function slot(name: string): Slot {
  const found = slots.find(item => item.name === name)
  if (!found)
    throw new Error(`slot ${name} is not registered`)
  return found
}

const blank = { render: () => null }
const router = vueRouter.createRouter({
  history: vueRouter.createMemoryHistory(),
  routes: [
    { path: '/', component: blank },
    { path: '/nginx_log/site', component: blank },
  ],
})

function mount(component: Component, props: Record<string, unknown>) {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = vue.createApp({
    render: () => vue.h(antdvNext.App, null, { default: () => vue.h(component, props) }),
  })
  app.use(router)
  app.mount(root)
  return { root, app }
}

suite('built bundles', () => {
  beforeAll(async () => {
    ;(globalThis as unknown as { WebSocket: unknown }).WebSocket = FakeWebSocket
    const host = window as unknown as { NginxUI: unknown }

    host.NginxUI = {
      version: 'test',
      shared: { vue, vueRouter, antdvNext, antdvIcons, gettext: gettextInstance(), versions: {} },
      registerPlugin: (_id: string, definition: { setup: (registry: unknown) => void }) => {
        plugin = definition
      },
      registerChunk: (_id: string, name: string, exports: Record<string, unknown>) => {
        chunkExports[name] = exports
      },
    }

    evaluate('main.js')

    const registry = {
      http,
      // The host builds the address of plugin websockets, credentials included.
      wsUrl: (path: string) => `wss://ui.test/api/plugins/${PLUGIN_ID}/http/${path.replace(/^\/+/, '')}?token=t&x_node_id=3`,
      host: vue.reactive({ theme: 'light', locale: 'zh_CN' }),
      manifest: {},
      registerSlot: (name: string, component: Component, options: Record<string, unknown> = {}) => slots.push({ name, component, options }),
      registerRoute: () => {},
      registerSettingsPanel: (component: Component) => {
        settingsPanel = component
      },
      registerTranslations: (locale: string, messages: Record<string, string>) => {
        translations[locale] = { ...translations[locale], ...messages }
      },
      loadChunk: async (name: string) => {
        evaluate(`chunks/${name}.js`)
        return chunkExports[name]
      },
    }
    loadChunk = registry.loadChunk
    plugin?.setup(registry)
    await flush()
  })

  afterAll(() => {
    delete (window as unknown as { NginxUI?: unknown }).NginxUI
  })

  test('registers itself under the plugin id and everything the contract lists', () => {
    expect(plugin).toBeDefined()
    expect(slots.map(item => item.name).sort()).toEqual([
      'nginx_log.list.column:document_count',
      'nginx_log.list.column:index_status',
      'nginx_log.list.column:last_indexed',
      'nginx_log.list.column:timerange',
      'nginx_log.list.row.actions',
      'nginx_log.list.toolbar',
      'nginx_log.view:dashboard',
      'nginx_log.view:structured',
      'site.log.actions',
    ])
    expect(settingsPanel).toBeDefined()
  })

  test('carries translations for every locale of the host', () => {
    expect(Object.keys(translations).sort()).toEqual(['ar', 'de_DE', 'es', 'fr_FR', 'ja_JP', 'ko_KR', 'pt_PT', 'ru_RU', 'tr_TR', 'uk_UA', 'vi_VN', 'zh_CN', 'zh_TW'])
    expect(translations.zh_CN['Index Status']).toBe('索引状态')
    expect(translations.ja_JP.Structured).toBe('構造化')
    expect(translations.zh_TW['Time Range']).toBe('時間範圍')
  })

  test('labels, sorting and filters are declared on the columns', () => {
    const status = slot('nginx_log.list.column:index_status')
    expect(status.options.label).toBe('Index Status')
    expect(typeof status.options.sortValue).toBe('function')
    expect((status.options.filters as unknown[]).length).toBe(5)
    expect(slot('nginx_log.list.column:timerange').options.sortValue).toBeUndefined()
    expect(slot('nginx_log.view:structured').options.label).toBe('Structured')
    expect(slot('nginx_log.view:dashboard').options.label).toBe('Dashboard')
  })

  test('the views are not offered for error logs, the toolbar only for access logs', () => {
    const when = slot('nginx_log.view:structured').options.when as (ctx: unknown) => boolean
    expect(when({ path: '/var/log/nginx/access.log', type: 'access' })).toBe(true)
    expect(when({ path: '/var/log/nginx/error.log', type: 'error' })).toBe(false)
    const toolbar = slot('nginx_log.list.toolbar').options.when as (ctx: unknown) => boolean
    expect(toolbar({ type: 'error' })).toBe(false)
    expect(slot('site.log.actions').options.when).toBeDefined()
  })

  test('a status cell shows the translated state from the shared status request', async () => {
    const { root, app } = mount(slot('nginx_log.list.column:index_status').component, {
      row: { path: '/var/log/nginx/access.log', type: 'access', name: 'access.log', config_file: '' },
    })
    await flush()
    await flush()

    expect(root.textContent).toContain('已索引')
    expect(requests.filter(item => item === 'GET /logs/status').length).toBe(1)
    // The socket address is the one the host built.
    await flush()
    const socket = FakeWebSocket.instances[0]
    expect(socket.url).toBe(`wss://ui.test/api/plugins/${PLUGIN_ID}/http/events?token=t&x_node_id=3`)

    const queued = mount(slot('nginx_log.list.column:index_status').component, {
      row: { path: '/var/log/nginx/other.log', type: 'access', name: 'other.log', config_file: '' },
    })
    await flush()
    expect(queued.root.textContent).toContain('#2')
    // Two cells share one request and one socket.
    expect(requests.filter(item => item === 'GET /logs/status').length).toBe(1)
    expect(FakeWebSocket.instances.length).toBe(1)

    const documents = mount(slot('nginx_log.list.column:document_count').component, {
      row: { path: '/var/log/nginx/access.log', type: 'access', name: 'access.log', config_file: '' },
    })
    await flush()
    expect(documents.root.textContent).toBe((1234).toLocaleString())

    app.unmount()
    queued.app.unmount()
    documents.app.unmount()
  })

  test('the toolbar shows the indexing state while a file is queued', async () => {
    const { root, app } = mount(slot('nginx_log.list.toolbar').component, { type: 'access' })
    await flush()

    expect(root.textContent).toContain('正在索引日志...')
    expect(root.textContent).not.toContain('重建所有索引')
    app.unmount()
  })

  test('the toolbar summarizes the index and offers the rebuild once indexing is over', async () => {
    statusBody.items[1].index_status = 'not_indexed'
    await useStatusStore().refresh()

    const { root, app } = mount(slot('nginx_log.list.toolbar').component, { type: 'access' })
    await flush()

    expect(root.textContent).toContain('已索引 1 / 2 个文件')
    expect(root.textContent).toContain('重建所有索引')
    app.unmount()

    const error = mount(slot('nginx_log.list.toolbar').component, { type: 'error' })
    await flush()
    expect(error.root.textContent).toBe('')
    error.app.unmount()
  })

  test('the structured view loads its chunk, warms the backend and renders', async () => {
    const { root, app } = mount(slot('nginx_log.view:structured').component, {
      path: '/var/log/nginx/access.log',
      type: 'access',
    })
    await flush()
    await flush()

    expect(requests).toContain('POST /warm')
    expect(Object.keys(chunkExports.search ?? {})).toEqual(['Structured'])
    expect(requests).toContain('GET /preflight')
    // The file is not indexed yet, so the viewer shows its indexing state.
    expect(root.textContent).toContain('...')
    expect(document.head.querySelector(`style[data-nginx-ui-chunk="${PLUGIN_ID}:search"]`)).not.toBeNull()
    app.unmount()
  })

  test('the dashboard chunk hands over its component and its styles', async () => {
    const exports = await loadChunk('dashboard')

    expect(Object.keys(exports)).toEqual(['Dashboard'])
    expect((exports.Dashboard as { props: Record<string, unknown> }).props).toHaveProperty('path')
    expect(document.head.querySelector(`style[data-nginx-ui-chunk="${PLUGIN_ID}:dashboard"]`)).not.toBeNull()
    // The dashboard view is loaded by the same wrapper as the structured one.
    expect(slot('nginx_log.view:dashboard').component).not.toBe(slot('nginx_log.view:structured').component)
  })

  test('the settings panel shows the fields with the saved values and saves through the host', async () => {
    let saved: Record<string, unknown> | undefined
    const { root, app } = mount(settingsPanel!, {
      settings: { incremental_index_interval: 30, max_concurrent_index_tasks: 0, index_custom_mmdb: '', geo_map_path: '/maps', other: 'kept' },
      save: async (next: Record<string, unknown>) => {
        saved = next
      },
    })
    await flush()

    expect(root.textContent).toContain('增量索引间隔')
    expect(root.textContent).toContain('GeoLite2 数据库')
    const inputs = root.querySelectorAll('input')
    expect((inputs[0] as HTMLInputElement).value).toBe('30')
    expect((inputs[3] as HTMLInputElement).value).toBe('/maps')
    expect(saved).toBeUndefined()
    app.unmount()
  })

  test('the site button shows only when the site has an access log', async () => {
    const button = slot('site.log.actions').component
    const context = { accessLogInherited: false, errorLogPath: '', errorLogInherited: false, siteName: 'site' }
    const withLog = mount(button, { ...context, accessLogPath: '/var/log/nginx/site.access.log' })
    await flush()
    expect(withLog.root.textContent).toContain('流量分析')

    const without = mount(button, { ...context, accessLogPath: '' })
    await flush()
    expect(without.root.textContent).not.toContain('流量分析')
    withLog.app.unmount()
    without.app.unmount()
  })

  test('the site button is offered for a path only, inherited or not', () => {
    const when = slot('site.log.actions').options.when as (ctx: unknown) => boolean
    expect(when({ accessLogPath: '/var/log/nginx/access.log', accessLogInherited: true })).toBe(true)
    expect(when({ accessLogPath: '', accessLogInherited: false })).toBe(false)
  })

  test('the site button warns before opening the analytics of an inherited default log', async () => {
    const button = slot('site.log.actions').component
    const context = { errorLogPath: '', errorLogInherited: false, siteName: 'site' }

    const inherited = mount(button, { ...context, accessLogPath: '/var/log/nginx/access.log', accessLogInherited: true })
    await flush()
    inherited.root.querySelector('button')!.click()
    await flush()
    // The host translates the notice with the strings this bundle registered.
    const notice = translations.zh_CN['This site uses the default access log, which may contain traffic from other sites']
    expect(notice).toBeDefined()
    expect(document.body.textContent).toContain(notice)
    expect(router.currentRoute.value.path).toBe('/nginx_log/site')
    expect(router.currentRoute.value.query).toEqual({ path: '/var/log/nginx/access.log', view: 'dashboard' })
    document.body.querySelectorAll('.ant-message').forEach(node => node.remove())

    const own = mount(button, { ...context, accessLogPath: '/var/log/nginx/site.access.log', accessLogInherited: false })
    await flush()
    own.root.querySelector('button')!.click()
    await flush()
    expect(document.body.textContent).not.toContain(notice)
    expect(router.currentRoute.value.query.path).toBe('/var/log/nginx/site.access.log')
    inherited.app.unmount()
    own.app.unmount()
  })

  test('the index columns are decided per list: access only', () => {
    for (const name of ['index_status', 'last_indexed', 'document_count', 'timerange']) {
      const when = slot(`nginx_log.list.column:${name}`).options.when as (ctx: unknown) => boolean
      expect(when({ type: 'access' })).toBe(true)
      expect(when({ type: 'error' })).toBe(false)
    }
  })
})
