<script setup lang="ts">
import type { NginxLogRow } from '@nginxui/plugin-sdk'
import { CheckCircleOutlined } from '@antdv-next/icons'
import { computed } from 'vue'
import { useStatus } from '@/store/status'
import { formatDuration, formatUnix } from '@/utils'

const props = defineProps<{ row: NginxLogRow }>()

const store = useStatus()
const item = computed(() => store.statusFor(props.row.path))
const lastIndexed = computed(() => item.value?.last_indexed ? formatUnix(item.value.last_indexed) : '')
const duration = computed(() => item.value?.index_duration ? `(${formatDuration(item.value.index_duration)})` : '')
</script>

<template>
  <span v-if="lastIndexed" style="white-space: nowrap">
    {{ lastIndexed }}
    <span v-if="duration" class="la-text-xs la-text-gray-500 dark:la-text-gray-400 la-ml-1">{{ duration }}</span>
    <CheckCircleOutlined class="la-text-green-500 la-ml-1" />
  </span>
  <span v-else class="la-text-gray-400 dark:la-text-gray-500">-</span>
</template>
