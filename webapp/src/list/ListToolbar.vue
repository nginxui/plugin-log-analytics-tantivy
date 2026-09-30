<script setup lang="ts">
import { SyncOutlined } from '@antdv-next/icons'
import { Button as AButton, App, Modal } from 'antdv-next'
import { computed, ref } from 'vue'
import { rebuildIndex } from '@/api/client'
import { errorMessage } from '@/errors'
import { $gettext } from '@/gettext'
import { useStatus } from '@/store/status'

defineProps<{ type: string }>()

const { message } = App.useApp()
const store = useStatus()

const loading = ref(false)
const indexing = computed(() => store.isIndexing())
const summary = computed(() => {
  const value = store.summary.value
  if (!value?.total_files)
    return ''

  return $gettext('%{indexed} of %{total} files indexed, %{documents} documents', {
    indexed: (value.indexed_files ?? 0).toLocaleString(),
    total: value.total_files.toLocaleString(),
    documents: (value.document_count ?? 0).toLocaleString(),
  })
})

function confirmRebuild() {
  Modal.confirm({
    title: $gettext('Rebuild Index'),
    content: $gettext('This will rebuild the entire log index. All existing index data will be deleted and rebuilt from scratch. This may take some time. Continue?'),
    okText: $gettext('Yes'),
    okType: 'danger',
    cancelText: $gettext('Cancel'),
    async onOk() {
      try {
        loading.value = true
        await rebuildIndex()
        message.success($gettext('Index and statistics rebuild started successfully'))
        await store.refresh()
      }
      catch (error) {
        message.error(errorMessage(error))
      }
      finally {
        loading.value = false
      }
    },
  })
}
</script>

<template>
  <div v-if="type === 'access'" class="la-flex la-items-center la-gap-4">
    <span v-if="summary && !indexing" class="la-text-sm la-text-gray-500 dark:la-text-gray-400">{{ summary }}</span>

    <!-- Global indexing progress -->
    <div v-if="indexing" class="la-flex la-items-center la-text-blue-500">
      <SyncOutlined spin class="la-mr-2" />
      <span>{{ $gettext('Indexing logs...') }}</span>
    </div>

    <AButton
      v-else
      type="link"
      size="small"
      :loading="loading"
      @click="confirmRebuild"
    >
      {{ $gettext('Rebuild All Index') }}
    </AButton>
  </div>
</template>
