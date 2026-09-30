<script setup lang="ts">
import type { NginxLogRow } from '@nginxui/plugin-sdk'
import { Button as AButton, App, Modal } from 'antdv-next'
import { rebuildFileIndex } from '@/api/client'
import { errorMessage } from '@/errors'
import { $gettext } from '@/gettext'
import { useStatus } from '@/store/status'

const props = defineProps<{ row: NginxLogRow }>()

const { message } = App.useApp()
const store = useStatus()

function confirmRebuild() {
  const path = props.row.path
  Modal.confirm({
    title: $gettext('Rebuild File Index'),
    content: $gettext('This will rebuild the index data for this specific file: %{path}', { path }),
    okText: $gettext('Yes'),
    okType: 'primary',
    cancelText: $gettext('Cancel'),
    async onOk() {
      try {
        await rebuildFileIndex(path)
        message.success($gettext('File index rebuild started successfully for %{path}', { path }))
        await store.refresh()
      }
      catch (error) {
        message.error(errorMessage(error))
      }
    },
  })
}
</script>

<template>
  <AButton
    v-if="row.type === 'access'"
    type="link"
    size="small"
    :disabled="store.isIndexing()"
    @click="confirmRebuild"
  >
    {{ $gettext('Rebuild') }}
  </AButton>
</template>
