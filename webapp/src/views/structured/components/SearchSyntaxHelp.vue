<script setup lang="ts">
import { QuestionCircleOutlined } from '@antdv-next/icons'
import { Popover as APopover } from 'antdv-next'
import { $gettext } from '@/gettext'
import { syntaxRows } from './search-syntax'
</script>

<template>
  <APopover placement="bottomLeft" :trigger="['hover', 'click']">
    <template #content>
      <div class="la-w-[22rem] la-max-w-full la-text-xs">
        <div class="la-font-medium la-mb-1">
          {{ $gettext('Search syntax') }}
        </div>
        <p class="la-mb-2 la-text-gray-600 dark:la-text-trueGray-400">
          {{ $gettext('Separate words and filters with spaces. A line has to match all of them.') }}
        </p>
        <div
          v-for="row in syntaxRows()"
          :key="row.examples[0]"
          class="la-py-1 la-border-t la-border-gray-200 dark:la-border-trueGray-700"
        >
          <div class="la-flex la-flex-wrap la-gap-x-2">
            <code
              v-for="example in row.examples"
              :key="example"
              class="la-px-1 la-rounded la-bg-gray-100 dark:la-bg-trueGray-700 la-text-gray-900 dark:la-text-trueGray-100"
            >{{ example }}</code>
          </div>
          <div class="la-text-gray-600 dark:la-text-trueGray-400">
            {{ row.meaning }}
          </div>
        </div>
      </div>
    </template>
    <QuestionCircleOutlined
      class="la-ml-1 la-cursor-pointer la-text-gray-500 dark:la-text-trueGray-400"
      :aria-label="$gettext('Search syntax')"
    />
  </APopover>
</template>
