<script setup lang="ts">
import type { NginxLogRow } from '@nginxui/plugin-sdk'
import { Badge, Tooltip } from 'antdv-next'
import { computed } from 'vue'
import IndexProgressBar from '@/components/IndexProgressBar.vue'
import { $gettext } from '@/gettext'
import { useStatus } from '@/store/status'

const props = defineProps<{ row: NginxLogRow }>()

const store = useStatus()
const item = computed(() => store.statusFor(props.row.path))
// A file that is being indexed shows its progress instead of the state badge
const progress = computed(() => store.progressFor(props.row.path))

const queueText = computed(() => {
  const position = item.value?.queue_position
  return position ? `${$gettext('Queued')} (#${position})` : $gettext('Queued')
})
</script>

<template>
  <div v-if="progress" style="min-width: 200px; padding: 6px 0 8px 0;">
    <IndexProgressBar :progress="progress" size="small" />
  </div>
  <template v-else>
    <Badge v-if="item?.index_status === 'indexed' || item?.index_status === 'ready'" status="success" :text="$gettext('Indexed')" />
    <Badge v-else-if="item?.index_status === 'indexing'" status="processing" :text="$gettext('Indexing')" />
    <Tooltip v-else-if="item?.index_status === 'error'" :title="item.error_message || $gettext('Index failed')">
      <Badge status="error" :text="$gettext('Error')" />
    </Tooltip>
    <Badge v-else-if="item?.index_status === 'queued'" status="processing" :text="queueText" />
    <Badge v-else status="default" :text="$gettext('Not Indexed')" />
  </template>
</template>
