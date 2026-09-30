<script setup lang="ts">
import type { NginxLogRow } from '@nginxui/plugin-sdk'
import { computed } from 'vue'
import { useStatus } from '@/store/status'
import { formatUnix } from '@/utils'
import { timerangeOf } from './columns'

const props = defineProps<{ row: NginxLogRow }>()

const store = useStatus()
const range = computed(() => {
  const item = store.statusFor(props.row.path)
  // Without has_timerange the range fields carry no meaning
  return item?.has_timerange === false ? undefined : timerangeOf(item)
})
</script>

<template>
  <span v-if="range" style="white-space: nowrap">{{ formatUnix(range.start, true) }} ~ {{ formatUnix(range.end, true) }}</span>
  <span v-else class="la-text-gray-400 dark:la-text-gray-500">-</span>
</template>
