<script setup lang="ts">
import { CheckCircleOutlined, DownloadOutlined, InfoCircleOutlined, WarningOutlined } from '@antdv-next/icons'
import {
  Alert as AAlert,
  Button as AButton,
  Divider as ADivider,
  Form as AForm,
  FormItem as AFormItem,
  Input as AInput,
  InputNumber as AInputNumber,
  TypographyText as ATypographyText,
  TypographyTitle as ATypographyTitle,
} from 'antdv-next'
import { computed, h, reactive, ref, watch } from 'vue'
import GeoLiteDownload from '@/components/GeoLiteDownload.vue'
import { $gettext } from '@/gettext'

const props = defineProps<{
  settings: Record<string, unknown>
  save: (next?: Record<string, unknown>) => Promise<void>
}>()

const saving = ref(false)

function numberOf(value: unknown): number {
  const parsed = Number(value)
  return Number.isFinite(parsed) && parsed > 0 ? parsed : 0
}

function readForm() {
  return {
    incremental_index_interval: numberOf(props.settings.incremental_index_interval),
    max_concurrent_index_tasks: numberOf(props.settings.max_concurrent_index_tasks),
    index_custom_mmdb: String(props.settings.index_custom_mmdb ?? ''),
    geo_map_path: String(props.settings.geo_map_path ?? ''),
  }
}

const form = reactive(readForm())

// The host reloads the values after a save, so the form follows them.
watch(() => props.settings, () => Object.assign(form, readForm()))

const dirty = computed(() => {
  const saved = readForm()
  return (Object.keys(saved) as Array<keyof typeof saved>).some(key => saved[key] !== form[key])
})

async function submit() {
  saving.value = true
  try {
    await props.save({ ...props.settings, ...form })
  }
  finally {
    saving.value = false
  }
}

const systemRequirements = [
  {
    title: $gettext('CPU'),
    requirement: $gettext('1 core minimum'),
    recommended: $gettext('2+ cores recommended'),
  },
  {
    title: $gettext('Memory'),
    requirement: $gettext('1GB RAM minimum'),
    recommended: $gettext('4GB+ RAM recommended'),
  },
  {
    title: $gettext('Storage'),
    requirement: $gettext('At least 20GB available disk space'),
    recommended: $gettext('SSD storage for better I/O performance'),
  },
]
</script>

<template>
  <div class="la-space-y-6">
    <AAlert
      :title="$gettext('Resource Usage Warning')"
      :description="$gettext('Log indexing consumes significant computational resources including CPU and memory. Please ensure your system meets the minimum requirements.')"
      type="warning"
      show-icon
      :icon="h(WarningOutlined)"
    />

    <!-- System Requirements -->
    <div>
      <ATypographyTitle :level="4" class="la-mb-3">
        <InfoCircleOutlined class="la-mr-2" />
        {{ $gettext('System Requirements') }}
      </ATypographyTitle>

      <div class="la-space-y-3">
        <div
          v-for="item in systemRequirements"
          :key="item.title"
          class="la-flex la-items-start la-space-x-3"
        >
          <CheckCircleOutlined class="la-text-green-500 la-mt-1" />
          <div class="la-space-y-1">
            <ATypographyText strong>
              {{ item.title }}
            </ATypographyText>
            <div>
              <ATypographyText>
                {{ $gettext('Minimum:') }}
              </ATypographyText>
              <ATypographyText type="secondary">
                {{ item.requirement }}
              </ATypographyText>
            </div>
            <div>
              <ATypographyText>
                {{ $gettext('Recommended:') }}
              </ATypographyText>
              <ATypographyText type="secondary">
                {{ item.recommended }}
              </ATypographyText>
            </div>
          </div>
        </div>
      </div>
    </div>

    <ADivider />

    <!-- Indexing settings -->
    <div>
      <ATypographyTitle :level="4" class="la-mb-3">
        {{ $gettext('Index Settings') }}
      </ATypographyTitle>

      <AForm layout="vertical" @finish="submit">
        <AFormItem
          :label="$gettext('Incremental index interval (minutes)')"
          :extra="$gettext('How often new log lines are added to the index. 0 uses the default of 15 minutes.')"
        >
          <AInputNumber v-model:value="form.incremental_index_interval" :min="0" :precision="0" style="width: 100%" />
        </AFormItem>

        <AFormItem
          :label="$gettext('Maximum concurrent index tasks')"
          :extra="$gettext('How many log groups are indexed at the same time. Lower it to use less memory while indexing. 0 chooses automatically from the available CPU.')"
        >
          <AInputNumber v-model:value="form.max_concurrent_index_tasks" :min="0" :precision="0" style="width: 100%" />
        </AFormItem>

        <AFormItem
          :label="$gettext('Custom MMDB file')"
          :extra="$gettext('Optional. A MaxMind DB file with your own location labels. Leave empty to use the GeoLite2 database.')"
        >
          <AInput v-model:value="form.index_custom_mmdb" />
        </AFormItem>

        <AFormItem
          :label="$gettext('Map files directory')"
          :extra="$gettext('Directory with the map outline files of China. Leave empty to use the plugin\'s own folder. Files not found there are loaded online.')"
        >
          <AInput v-model:value="form.geo_map_path" />
        </AFormItem>

        <AButton type="primary" html-type="submit" :loading="saving" :disabled="!dirty">
          {{ $gettext('Save') }}
        </AButton>
      </AForm>
    </div>

    <ADivider />

    <!-- GeoLite Database Section -->
    <div>
      <ATypographyTitle :level="4" class="la-mb-3">
        <DownloadOutlined class="la-mr-2 la-text-purple-500" />
        {{ $gettext('GeoLite2 Database') }}
      </ATypographyTitle>

      <GeoLiteDownload />
    </div>
  </div>
</template>
