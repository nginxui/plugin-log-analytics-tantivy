import { defineComponent, h } from 'vue'
import DashboardViewer from './views/dashboard/DashboardViewer.vue'
// Entry of the "dashboard" chunk: the traffic dashboard with its charts and maps.
import 'virtual:uno.css'

// The host passes the slot context as props, the viewer names the path logPath.
export const Dashboard = defineComponent({
  name: 'LogAnalyticsDashboard',
  props: {
    path: { type: String, default: '' },
  },
  setup(props) {
    return () => h(DashboardViewer, { logPath: props.path })
  },
})
