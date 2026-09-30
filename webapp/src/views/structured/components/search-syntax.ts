// The search box syntax that the help lists. The examples are checked against
// the parser of the plugin process, so they have to stay valid filters.
import { $gettext } from '@/gettext'

export interface SyntaxRow {
  /** Examples as typed, never translated. */
  examples: string[]
  meaning: string
}

export function syntaxRows(): SyntaxRow[] {
  return [
    { examples: ['status:404', 'status:5xx', 'status:400-499'], meaning: $gettext('Status code, status class or range of codes') },
    { examples: ['method:POST'], meaning: $gettext('Request method') },
    { examples: ['ip:192.168.0.0/16', 'ip:2001:db8::/32'], meaning: $gettext('Client address or network') },
    { examples: ['path:/api/'], meaning: $gettext('Words of the request path, in order') },
    { examples: ['ua:curl', 'referer:google'], meaning: $gettext('Words of the user agent or of the referer, in order') },
    { examples: ['browser:Chrome', 'os:Linux', 'device:mobile'], meaning: $gettext('Browser, operating system or device type') },
    { examples: ['country:CN', 'region:Bavaria', 'city:Berlin'], meaning: $gettext('Country code, province or city of the client') },
    { examples: ['bytes:>1000', 'rt:>0.5', 'rt:0.1..0.5'], meaning: $gettext('Bytes sent or request time in seconds, with a comparison or a range') },
    { examples: ['-bot', '-status:404'], meaning: $gettext('Leaves out the lines that match') },
    { examples: ['"union select"'], meaning: $gettext('Words that belong together') },
  ]
}

export interface QueryWarning {
  token: string
  reason: string
}

/** A sentence for a part of the search box that was read as plain text or left out. */
export function warningText(warning: QueryWarning): string {
  const token = warning.token
  switch (warning.reason) {
    case 'unknown_field':
      return $gettext('"%{token}" does not name a known field and was searched as text.', { token })
    case 'invalid_value':
      return $gettext('The value in "%{token}" is not valid for this field and was searched as text.', { token })
    case 'unterminated_quote':
      return $gettext('The quotation mark in "%{token}" is not closed and was searched as text.', { token })
    case 'too_many_terms':
      return $gettext('The search is too long. Only the first 64 terms were used.')
    default:
      return $gettext('"%{token}" was searched as text.', { token })
  }
}
