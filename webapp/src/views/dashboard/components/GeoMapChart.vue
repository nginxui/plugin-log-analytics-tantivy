<script setup lang="ts">
import type { ChinaMapData, WorldMapData } from '@/api/types'
import { Card as ACard, Segmented as ASegmented } from 'antdv-next'
import { computed, ref, watch } from 'vue'
import { $gettext } from '@/gettext'
import { useGeoTranslation } from '../geo'
import ChinaMapChart from './ChinaMapChart/ChinaMapChart.vue'
import WorldMapChart from './WorldMapChart/WorldMapChart.vue'

const props = defineProps<{
  worldData: WorldMapData[] | null
  chinaData: ChinaMapData[] | null
  enableChinaMap?: boolean
  loading: boolean
  logPath: string
  startTime: number
  endTime: number
}>()

const { isChineseLocale } = useGeoTranslation()

// Map type selection - default to global, only allow china for Chinese locales
const mapType = ref<'global' | 'china'>('global')
const canShowChinaMap = computed(() => isChineseLocale.value && Boolean(props.enableChinaMap))

// Watch language changes and reset to global if switching from Chinese to non-Chinese
watch(canShowChinaMap, newVal => {
  if (!newVal && mapType.value === 'china') {
    mapType.value = 'global'
  }
})

// Segment options - only show China option for Chinese locales
const segmentOptions = computed(() => {
  const options = [{ label: $gettext('Global Map'), value: 'global' }]

  if (canShowChinaMap.value) {
    options.push({ label: $gettext('China Map'), value: 'china' })
  }

  return options
})

// Card title
const cardTitle = computed(() => {
  return mapType.value === 'global' ? $gettext('Global Access Map') : $gettext('China Access Map')
})
</script>

<template>
  <ACard :loading="loading">
    <template #title>
      <div class="la-flex la-items-center la-justify-between">
        <span>{{ cardTitle }}</span>
        <ASegmented
          v-if="canShowChinaMap"
          v-model:value="mapType"
          :options="segmentOptions"
        />
      </div>
    </template>

    <div class="geo-map-container">
      <!-- World Map -->
      <div v-show="mapType === 'global'">
        <WorldMapChart
          :data="props.worldData"
          :loading="props.loading"
          @drill-china="canShowChinaMap && (mapType = 'china')"
        />
      </div>

      <!-- China Map, its outlines are only fetched once it can be shown -->
      <div v-if="canShowChinaMap" v-show="mapType === 'china'">
        <ChinaMapChart
          :data="props.chinaData"
          :loading="props.loading"
          :log-path="props.logPath"
          :start-time="props.startTime"
          :end-time="props.endTime"
        />
      </div>
    </div>
  </ACard>
</template>

<style scoped>
.geo-map-container {
  min-height: 400px;
}
</style>
