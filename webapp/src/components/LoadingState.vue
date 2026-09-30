<script setup lang="ts">
import type { CSSProperties } from 'vue'
import { LoadingOutlined } from '@antdv-next/icons'
import { computed, markRaw } from 'vue'
import { $gettext } from '@/gettext'
import { useStatusStore } from '@/store/status'
import IndexProgressBar from './IndexProgressBar.vue'

interface IndexStatusDetails {
  available: boolean
  index_status: string
  message?: string
}

const props = defineProps<{
  logPath?: string
  size?: 'small' | 'default' | 'large'
  indexStatus?: IndexStatusDetails
}>()

// Index progress tracking
const store = useStatusStore()
const indexProgress = computed(() => props.logPath ? store.progressFor(props.logPath) ?? null : null)
const isCurrentFileIndexing = computed(() => props.logPath ? store.isFileIndexing(props.logPath) : false)

// Status-based loading message and icon
const statusInfo = computed(() => {
  const status = props.indexStatus?.index_status

  switch (status) {
    case 'indexing':
      return {
        icon: markRaw(LoadingOutlined),
        message: $gettext('Indexing logs...'),
        color: 'la-text-blue-500',
        showProgress: true,
      }
    case 'indexed':
    case 'ready': // Treat 'ready' as 'indexed'
      return {
        icon: markRaw(LoadingOutlined),
        message: $gettext('Loading...'),
        color: 'la-text-green-500',
        showProgress: false,
      }
    case 'queued':
      return {
        icon: markRaw(LoadingOutlined),
        message: $gettext('Queued for indexing...'),
        color: 'la-text-orange-500',
        showProgress: false,
      }
    case 'error':
      return {
        icon: markRaw(LoadingOutlined),
        message: $gettext('Index failed, please try rebuilding'),
        color: 'la-text-red-500',
        showProgress: false,
      }
    case 'not_indexed':
      return {
        icon: markRaw(LoadingOutlined),
        message: $gettext('Log file not indexed yet'),
        color: 'la-text-gray-500',
        showProgress: false,
      }
    default:
      // Fallback for active indexing check
      if (isCurrentFileIndexing.value) {
        return {
          icon: markRaw(LoadingOutlined),
          message: $gettext('Indexing...'),
          color: 'la-text-blue-500',
          showProgress: true,
        }
      }
      return {
        icon: markRaw(LoadingOutlined),
        message: $gettext('Loading...'),
        color: 'la-text-blue-500',
        showProgress: false,
      }
  }
})

const iconClass = computed(() => {
  const baseColor = statusInfo.value.color || 'la-text-blue-500'
  switch (props.size) {
    case 'small':
      return `la-text-lg ${baseColor}`
    case 'large':
      return `la-text-4xl ${baseColor}`
    default:
      return `la-text-2xl ${baseColor}`
  }
})

const containerStyle = computed((): CSSProperties => {
  let height = '50vh' // Default responsive height
  let padding = '40px'

  switch (props.size) {
    case 'small':
      height = '30vh'
      padding = '20px'
      break
    case 'large':
      height = '70vh'
      padding = '60px'
      break
  }

  return {
    minHeight: height,
    padding,
    display: 'flex',
    flexDirection: 'column' as const,
    justifyContent: 'center',
    alignItems: 'center',
  }
})
</script>

<template>
  <div :style="containerStyle">
    <!-- Status Icon -->
    <component :is="statusInfo.icon" :class="iconClass" />

    <!-- Progress Bar (only show when actively indexing or if progress data exists) -->
    <div v-if="(statusInfo.showProgress && indexProgress) || indexProgress" class="la-mt-4 la-flex la-flex-col la-items-center">
      <div class="la-max-w-75 la-w-full">
        <IndexProgressBar
          :progress="indexProgress"
          size="small"
        />
      </div>
    </div>

    <!-- Status Message -->
    <p class="la-mt-4">
      {{ statusInfo.message }}
    </p>
  </div>
</template>
