import { defineComponent, h } from 'vue'
import { logKindOf } from './rules'
import StructuredLogViewer from './views/structured/StructuredLogViewer.vue'
// Entry of the "search" chunk: the structured log viewer.
import 'virtual:uno.css'

// The host passes the slot context as props, the viewer names the path logPath.
export const Structured = defineComponent({
  name: 'LogAnalyticsStructured',
  props: {
    path: { type: String, default: '' },
    type: { type: String, default: '' },
  },
  setup(props) {
    return () => h(StructuredLogViewer, {
      // A new kind of log starts a fresh viewer with its own filters
      key: logKindOf({ path: props.path, type: props.type }),
      logPath: props.path,
      logType: logKindOf({ path: props.path, type: props.type }),
    })
  },
})
