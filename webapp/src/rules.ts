import type { SlotContext } from '@nginxui/plugin-sdk'
import { isErrorLogPath } from './utils'

export type LogKind = 'access' | 'error'

/** Kind of the log a slot names: its type, or its path for another type. */
export function logKindOf(context: SlotContext): LogKind {
  if (context.type === 'error' || context.type === 'access')
    return context.type
  const path = typeof context.path === 'string' ? context.path : ''
  return isErrorLogPath(path) ? 'error' : 'access'
}

/** The dashboard only exists for access logs. */
export function isDashboardAvailable(context: SlotContext): boolean {
  return logKindOf(context) === 'access'
}
