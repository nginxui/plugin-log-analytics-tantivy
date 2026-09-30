<script setup lang="ts">
import { AreaChartOutlined } from '@antdv-next/icons'
import { Button as AButton, message } from 'antdv-next'
import { onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { warm } from '@/api/client'
import { $gettext } from '@/gettext'

const props = defineProps<{
  accessLogPath: string
  // True when the path is the nginx default log the site falls back to
  accessLogInherited?: boolean
  errorLogPath?: string
  errorLogInherited?: boolean
  siteName?: string
}>()

const router = useRouter()

// Warm the search shards while the person is still looking at the site
onMounted(() => warm())

function openAnalytics() {
  if (props.accessLogInherited)
    message.info($gettext('This site uses the default access log, which may contain traffic from other sites'))

  router.push({
    path: '/nginx_log/site',
    query: {
      path: props.accessLogPath,
      view: 'dashboard',
    },
  })
}
</script>

<template>
  <AButton
    v-if="accessLogPath"
    type="link"
    size="small"
    @click="openAnalytics"
  >
    <AreaChartOutlined />
    {{ $gettext('Traffic Analytics') }}
  </AButton>
</template>
