// Slot registrations of the entry bundle. Kept apart from main.ts so the rules
// (when a view is offered, what the columns sort and filter by) can be tested.
import type { NginxLogRow, PluginRegistry } from '@nginxui/plugin-sdk'
import { createChunkView } from './chunk'
import { N_ } from './gettext'
import { createSortValues, createStatusFilters, showsIndexColumns } from './list/columns'
import DocumentCountCell from './list/DocumentCountCell.vue'
import IndexStatusCell from './list/IndexStatusCell.vue'
import LastIndexedCell from './list/LastIndexedCell.vue'
import ListToolbar from './list/ListToolbar.vue'
import RebuildAction from './list/RebuildAction.vue'
import SettingsPanel from './list/SettingsPanel.vue'
import SiteAnalyticsButton from './list/SiteAnalyticsButton.vue'
import TimeRangeCell from './list/TimeRangeCell.vue'
import { isViewAvailable } from './rules'
import { useStatusStore } from './store/status'

export function registerSlots(registry: PluginRegistry): void {
  const getLoader = () => registry.loadChunk

  registry.registerSlot('nginx_log.view:structured', createChunkView({
    chunk: 'search',
    exportName: 'Structured',
    getLoader,
  }), { label: N_('Structured'), order: 10, when: isViewAvailable })

  registry.registerSlot('nginx_log.view:dashboard', createChunkView({
    chunk: 'dashboard',
    exportName: 'Dashboard',
    getLoader,
  }), { label: N_('Dashboard'), order: 20, when: isViewAvailable })

  // The four index columns read the shared status store and render its state.
  const store = useStatusStore()
  const sortValues = createSortValues(store)
  const columnRule = { when: showsIndexColumns }

  registry.registerSlot('nginx_log.list.column:index_status', IndexStatusCell, {
    ...columnRule,
    label: N_('Index Status'),
    order: 10,
    sortValue: sortValues.index_status,
    filters: createStatusFilters(store),
  })
  registry.registerSlot('nginx_log.list.column:last_indexed', LastIndexedCell, {
    ...columnRule,
    label: N_('Last Indexed'),
    order: 20,
    sortValue: sortValues.last_indexed,
  })
  registry.registerSlot('nginx_log.list.column:document_count', DocumentCountCell, {
    ...columnRule,
    label: N_('Document Count'),
    order: 30,
    sortValue: sortValues.document_count,
  })
  registry.registerSlot('nginx_log.list.column:timerange', TimeRangeCell, {
    ...columnRule,
    label: N_('Time Range'),
    order: 40,
  })

  registry.registerSlot('nginx_log.list.row.actions', RebuildAction, {
    when: ctx => (ctx.row as NginxLogRow | undefined)?.type !== 'error',
  })
  registry.registerSlot('nginx_log.list.toolbar', ListToolbar, {
    when: ctx => ctx.type === undefined || ctx.type === 'access',
  })
  registry.registerSlot('site.log.actions', SiteAnalyticsButton, {
    when: ctx => Boolean(ctx.accessLogPath),
  })

  registry.registerSettingsPanel(SettingsPanel)
}
