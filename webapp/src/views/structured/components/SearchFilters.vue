<script setup lang="ts">
import type { SearchFilters } from '@/api/types'
import type { LogKind } from '@/rules'
import { CaretRightOutlined } from '@antdv-next/icons'
import { Button as AButton, Input as AInput, Select as ASelect } from 'antdv-next'
import { ref } from 'vue'
import { $gettext } from '@/gettext'
import { browserOptions, deviceOptions, emptySearchFilters, levelOptions, methodOptions, osOptions, statusOptions } from './search-filter-options'
import SearchSyntaxHelp from './SearchSyntaxHelp.vue'

const { kind = 'access' } = defineProps<{ kind?: LogKind }>()

const emit = defineEmits<Emits>()

// Emits
interface Emits {
  (e: 'search'): void
  (e: 'reset'): void
}

// Use defineModel for simplified v-model handling
const filters = defineModel<SearchFilters>({ required: true })

// Collapse state
const collapsed = ref(true)

function handleSearch() {
  emit('search')
}

function handleReset() {
  filters.value = emptySearchFilters()
  emit('reset')
}
</script>

<template>
  <div class="la-bg-gray-50 dark:la-bg-trueGray-800 la-rounded la-border la-border-gray-200 dark:la-border-trueGray-700">
    <!-- Header -->
    <div
      class="la-px-4 la-py-3 la-cursor-pointer hover:la-bg-gray-100 dark:hover:la-bg-trueGray-700 la-flex la-items-center la-justify-between"
      @click="collapsed = !collapsed"
    >
      <div class="la-flex la-items-center la-space-x-2 la-min-h-[1.5rem]">
        <CaretRightOutlined
          class="la-transition-transform la-text-sm la-flex-shrink-0 la-leading-6" :class="[collapsed ? '' : 'la-rotate-90']"
        />
        <div class="la-text-sm la-font-medium la-text-gray-900 dark:la-text-trueGray-100 la-leading-6">
          {{ $gettext('Search Filters') }}
        </div>
      </div>
    </div>

    <!-- Content -->
    <div v-show="!collapsed" class="la-p-4 la-space-y-4 la-border-t la-border-gray-200 dark:la-border-trueGray-700">
      <!-- Row 1: Basic Search -->
      <div class="la-grid la-grid-cols-1 lg:la-grid-cols-3 la-gap-3">
        <!-- Full Text Search -->
        <div class="lg:la-col-span-2">
          <label class="la-block la-text-xs la-font-medium la-text-gray-700 dark:la-text-trueGray-300 la-mb-1">
            {{ $gettext('Full Text Search') }}
            <SearchSyntaxHelp :kind="kind" />
          </label>
          <AInput
            v-model:value="filters.query"
            :placeholder="$gettext('Search in log content...')"
            @press-enter="handleSearch"
          />
        </div>

        <!-- IP Address -->
        <div>
          <label class="la-block la-text-xs la-font-medium la-text-gray-700 dark:la-text-trueGray-300 la-mb-1">
            {{ $gettext('IP Address') }}
          </label>
          <AInput
            v-model:value="filters.ip"
            placeholder="192.168.1.1"
            @press-enter="handleSearch"
          />
        </div>
      </div>

      <!-- Error log: level and request path -->
      <div v-if="kind === 'error'" class="la-grid la-grid-cols-1 md:la-grid-cols-3 la-gap-3">
        <div>
          <label class="la-block la-text-xs la-font-medium la-text-gray-700 dark:la-text-trueGray-300 la-mb-1">
            {{ $gettext('Level') }}
          </label>
          <ASelect
            v-model:value="filters.level"
            mode="multiple"
            :placeholder="$gettext('Any')"
            allow-clear
            style="width: 100%"
            :options="levelOptions"
          />
        </div>
        <div class="md:la-col-span-2">
          <label class="la-block la-text-xs la-font-medium la-text-gray-700 dark:la-text-trueGray-300 la-mb-1">
            {{ $gettext('Request Path') }}
          </label>
          <AInput
            v-model:value="filters.path"
            placeholder="/"
            @press-enter="handleSearch"
          />
        </div>
      </div>

      <!-- Row 2: Request Details -->
      <div v-if="kind === 'access'" class="la-grid la-grid-cols-1 md:la-grid-cols-2 lg:la-grid-cols-4 la-gap-3">
        <!-- HTTP Method -->
        <div>
          <label class="la-block la-text-xs la-font-medium la-text-gray-700 dark:la-text-trueGray-300 la-mb-1">
            {{ $gettext('Method') }}
          </label>
          <ASelect
            v-model:value="filters.method"
            :placeholder="$gettext('Any')"
            allow-clear
            style="width: 100%"
            :options="methodOptions"
          />
        </div>

        <!-- Status Codes -->
        <div>
          <label class="la-block la-text-xs la-font-medium la-text-gray-700 dark:la-text-trueGray-300 la-mb-1">
            {{ $gettext('Status') }}
          </label>
          <ASelect
            v-model:value="filters.status"
            mode="tags"
            :placeholder="$gettext('Type or select status codes')"
            allow-clear
            style="width: 100%"
            :options="statusOptions"
            :token-separators="[',', ' ']"
          />
        </div>

        <!-- Request Path -->
        <div class="md:la-col-span-2">
          <label class="la-block la-text-xs la-font-medium la-text-gray-700 dark:la-text-trueGray-300 la-mb-1">
            {{ $gettext('Request Path') }}
          </label>
          <AInput
            v-model:value="filters.path"
            placeholder="/"
            @press-enter="handleSearch"
          />
        </div>
      </div>

      <!-- Row 3: Client Info -->
      <div v-if="kind === 'access'" class="la-grid la-grid-cols-1 md:la-grid-cols-2 lg:la-grid-cols-4 la-gap-3">
        <!-- Browser -->
        <div>
          <label class="la-block la-text-xs la-font-medium la-text-gray-700 dark:la-text-trueGray-300 la-mb-1">
            {{ $gettext('Browser') }}
          </label>
          <ASelect
            v-model:value="filters.browser"
            mode="tags"
            :placeholder="$gettext('Type or select browser')"
            allow-clear
            style="width: 100%"
            :options="browserOptions"
            :token-separators="[',', ' ']"
          />
        </div>

        <!-- Operating System -->
        <div>
          <label class="la-block la-text-xs la-font-medium la-text-gray-700 dark:la-text-trueGray-300 la-mb-1">
            {{ $gettext('OS') }}
          </label>
          <ASelect
            v-model:value="filters.os"
            mode="tags"
            :placeholder="$gettext('Type or select OS')"
            allow-clear
            style="width: 100%"
            :options="osOptions"
            :token-separators="[',', ' ']"
          />
        </div>

        <!-- Device Type -->
        <div>
          <label class="la-block la-text-xs la-font-medium la-text-gray-700 dark:la-text-trueGray-300 la-mb-1">
            {{ $gettext('Device') }}
          </label>
          <ASelect
            v-model:value="filters.device"
            mode="tags"
            :placeholder="$gettext('Type or select device')"
            allow-clear
            style="width: 100%"
            :options="deviceOptions"
            :token-separators="[',', ' ']"
          />
        </div>

        <!-- Referer -->
        <div>
          <label class="la-block la-text-xs la-font-medium la-text-gray-700 dark:la-text-trueGray-300 la-mb-1">
            {{ $gettext('Referer') }}
          </label>
          <AInput
            v-model:value="filters.referer"
            :placeholder="$gettext('https://...')"
            @press-enter="handleSearch"
          />
        </div>
      </div>

      <!-- Row 4: Advanced -->
      <div v-if="kind === 'access'" class="la-grid la-grid-cols-1 la-gap-3">
        <!-- User Agent -->
        <div>
          <label class="la-block la-text-xs la-font-medium la-text-gray-700 dark:la-text-trueGray-300 la-mb-1">
            {{ $gettext('User Agent') }}
          </label>
          <AInput
            v-model:value="filters.user_agent"
            :placeholder="$gettext('Mozilla/5.0...')"
            @press-enter="handleSearch"
          />
        </div>
      </div>

      <!-- Action Buttons -->
      <div class="la-flex la-items-center la-pt-3 la-border-t la-border-gray-200 dark:la-border-trueGray-700 la-justify-end">
        <div class="la-flex la-space-x-2">
          <AButton @click="handleReset">
            {{ $gettext('Reset') }}
          </AButton>
          <AButton type="primary" @click="handleSearch">
            {{ $gettext('Search') }}
          </AButton>
        </div>
      </div>
    </div>
  </div>
</template>
