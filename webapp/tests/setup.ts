// A DOM for the tests that mount components. It has to exist before vue is
// imported: the runtime looks up `document` once, when it loads.
import { GlobalRegistrator } from '@happy-dom/global-registrator'

GlobalRegistrator.register()

// happy-dom starts on about:blank, where relative URLs cannot be resolved.
;(window as unknown as { happyDOM: { setURL: (url: string) => void } }).happyDOM.setURL('http://localhost:9000/')
