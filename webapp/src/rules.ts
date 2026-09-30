import type { SlotContext } from '@nginxui/plugin-sdk'
import { isErrorLogPath } from './utils'

/** The structured and dashboard views only exist for access logs. */
export function isViewAvailable(context: SlotContext): boolean {
  const path = typeof context.path === 'string' ? context.path : ''
  if (context.type === 'error')
    return false
  if (context.type === 'access')
    return true
  return !isErrorLogPath(path)
}
