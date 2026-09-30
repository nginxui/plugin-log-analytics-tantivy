<script setup lang="ts">
import type { Ref } from 'vue'
import { CheckCircleOutlined, DownloadOutlined, InfoCircleOutlined } from '@antdv-next/icons'
import { Alert as AAlert, Button as AButton, Progress as AProgress, TypographyText as ATypographyText } from 'antdv-next'
import { computed, h, onBeforeUnmount, onMounted, ref } from 'vue'
import { getGeoLiteStatus } from '@/api/client'
import { $gettext } from '@/gettext'
import { openPluginSocket } from '@/host'
import { formatDateTime } from '@/utils'

interface Emits {
  (e: 'downloadComplete'): void
}

const props = withDefaults(defineProps<{
  hideRedownload?: boolean
}>(), {
  hideRedownload: false,
})

const emit = defineEmits<Emits>()

// GeoLite database state
const geoLiteStatus = ref({
  exists: false,
  path: '',
  size: 0,
  last_modified: '',
})
const geoLiteLoading = ref(false)
const downloading = ref(false)
const downloadProgress = ref(0)
const downloadStatus = ref('active') as Ref<'normal' | 'active' | 'success' | 'exception'>
const downloadMessage = ref('')
let activeSocket: WebSocket | undefined

const progressStrokeColor = {
  from: '#108ee9',
  to: '#87d068',
}

const downloadProgressComputed = computed(() => {
  return Number.parseFloat(downloadProgress.value.toFixed(1))
})

const lastUpdated = computed(() => {
  const date = new Date(geoLiteStatus.value.last_modified)
  return Number.isNaN(date.getTime()) ? '' : formatDateTime(date, true)
})

// Check GeoLite database status
async function checkGeoLiteStatus() {
  try {
    geoLiteLoading.value = true
    geoLiteStatus.value = await getGeoLiteStatus()
  }
  catch (e) {
    console.error('Failed to check GeoLite status:', e)
  }
  finally {
    geoLiteLoading.value = false
  }
}

// Download GeoLite database
async function downloadGeoLiteDB() {
  downloading.value = true
  downloadStatus.value = 'active'
  downloadProgress.value = 0
  downloadMessage.value = $gettext('Starting download...')

  let socket: WebSocket
  try {
    socket = await openPluginSocket('/geolite/download')
  }
  catch {
    downloadStatus.value = 'exception'
    downloadMessage.value = $gettext('Download failed')
    downloading.value = false
    return
  }
  activeSocket = socket

  let isFailed = false
  let currentPhase = 'download' // 'download' or 'decompress'

  socket.onopen = () => {
    socket.send('start')
  }

  socket.onmessage = async m => {
    const r = JSON.parse(m.data)

    // Update message and detect phase changes
    if (r.message) {
      downloadMessage.value = r.message

      // Detect phase transition
      if (r.message.toLowerCase().includes('decompress')) {
        currentPhase = 'decompress'
      }
    }

    switch (r.status) {
      case 'info':
        // Info messages handled above
        break
      case 'progress': {
        // Map progress to correct range based on phase
        const actualProgress = currentPhase === 'download'
          ? (r.progress / 100) * 50 // Download phase: 0-50%
          : 50 + (r.progress / 100) * 50 // Decompress phase: 50-100%

        downloadProgress.value = Math.min(actualProgress, 100)
        break
      }
      case 'error':
        downloadStatus.value = 'exception'
        isFailed = true
        break
      default:
        break
    }
  }

  socket.onerror = () => {
    isFailed = true
    downloadStatus.value = 'exception'
    downloadMessage.value = $gettext('Download failed')
  }

  socket.onclose = async () => {
    activeSocket = undefined
    if (isFailed) {
      downloading.value = false
      return
    }

    downloadStatus.value = 'success'
    downloadProgress.value = 100
    downloadMessage.value = $gettext('Download complete')

    // Refresh status
    await checkGeoLiteStatus()

    // Emit completion event
    emit('downloadComplete')

    // Reset after 2 seconds
    setTimeout(() => {
      downloading.value = false
      downloadProgress.value = 0
      downloadMessage.value = ''
    }, 2000)
  }
}

// Auto-check status on mount
onMounted(() => {
  checkGeoLiteStatus()
})

onBeforeUnmount(() => {
  if (activeSocket) {
    activeSocket.onclose = null
    activeSocket.close()
  }
})

// Expose methods for parent components
defineExpose({
  checkGeoLiteStatus,
  downloadGeoLiteDB,
})
</script>

<template>
  <div>
    <AAlert
      v-if="!geoLiteStatus.exists && !downloading"
      :title="$gettext('GeoLite2 Database Required')"
      type="info"
      show-icon
      :icon="h(InfoCircleOutlined)"
      class="la-mb-3"
    >
      <template #description>
        <div class="la-space-y-2">
          <p>{{ $gettext('The GeoLite2 database is required for offline geographic IP analysis. Please download it to enable this feature.') }}</p>
          <p class="la-text-sm">
            {{ $gettext('Alternatively, if you cannot download the database, you can place GeoLite2-City.mmdb in the geolite folder of the plugin data directory.') }}
          </p>
          <p class="la-text-sm">
            {{ $gettext('To use custom MMDB data, set the custom MMDB file in the plugin settings.') }}
          </p>
        </div>
      </template>
    </AAlert>

    <AAlert
      v-else-if="geoLiteStatus.exists && !downloading"
      :title="$gettext('GeoLite2 Database Installed')"
      type="success"
      show-icon
      :icon="h(CheckCircleOutlined)"
      banner
    />

    <br>

    <div class="la-space-y-3">
      <!-- Download Button -->
      <div class="la-flex la-items-center la-space-x-3">
        <AButton
          v-if="!geoLiteStatus.exists"
          type="primary"
          :loading="geoLiteLoading"
          :disabled="downloading"
          @click="downloadGeoLiteDB"
        >
          <DownloadOutlined />
          {{ $gettext('Download GeoLite2 Database') }}
        </AButton>
        <AButton
          v-else-if="!props.hideRedownload"
          :loading="geoLiteLoading"
          :disabled="downloading"
          @click="downloadGeoLiteDB"
        >
          <DownloadOutlined />
          {{ $gettext('Re-download Database') }}
        </AButton>
        <ATypographyText v-if="geoLiteStatus.exists && !downloading" type="secondary" class="la-text-xs">
          {{ $gettext('Last updated:') }} {{ lastUpdated }}
        </ATypographyText>
      </div>

      <!-- Inline Progress Bar -->
      <div v-if="downloading" class="download-progress-section">
        <AProgress
          :stroke-color="progressStrokeColor"
          :percent="downloadProgressComputed"
          :status="downloadStatus"
        />
      </div>
    </div>
  </div>
</template>
