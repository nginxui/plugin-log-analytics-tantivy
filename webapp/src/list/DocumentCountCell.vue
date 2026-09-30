<script setup lang="ts">
import type { NginxLogRow } from '@nginxui/plugin-sdk'
import { computed } from 'vue'
import { useStatus } from '@/store/status'

const props = defineProps<{ row: NginxLogRow }>()

const store = useStatus()
const count = computed(() => store.statusFor(props.row.path)?.document_count ?? 0)
</script>

<template>
  <span v-if="count">{{ count.toLocaleString() }}</span>
  <span v-else class="la-text-gray-400 dark:la-text-gray-500">-</span>
</template>
