<script setup lang="ts">
import { computed, ref } from 'vue';
import { useI18n } from 'vue-i18n';

import BaseButton from '@/shared/ui/BaseButton.vue';

import { type DroppedItem, groupDropped, toCsv } from '../dropped';

const props = defineProps<{ items: DroppedItem[] }>();

const SHOWN_IDS = 1000;
const { t } = useI18n();
const idsOpen = ref(false);

const groups = computed(() => groupDropped(props.items));
const shownItems = computed(() => props.items.slice(0, SHOWN_IDS));

function reasonOf(item: DroppedItem): string {
  return item.reason === 'cascade' && item.cascadeFrom !== null
    ? t('convert.dropped.reasonOf_cascade', { id: item.cascadeFrom })
    : t(`convert.dropped.reason_${item.reason}`);
}

function downloadCsv() {
  const url = URL.createObjectURL(new Blob([toCsv(props.items)], { type: 'text/csv;charset=utf-8' }));
  const link = document.createElement('a');
  link.href = url;
  link.download = 'dropped-elements.csv';
  link.click();
  URL.revokeObjectURL(url);
}
</script>

<template>
  <section class="dropped">
    <h2 class="title">
      <span>{{ t('convert.dropped.title', { n: items.length }, items.length) }}</span>
      <template v-if="items.length > 0">
        <BaseButton variant="link" @click="downloadCsv">{{ t('convert.dropped.csv') }}</BaseButton>
      </template>
    </h2>
    <table v-if="items.length > 0">
      <thead>
        <tr>
          <th scope="col">{{ t('convert.dropped.class') }}</th>
          <th scope="col">{{ t('convert.dropped.reason') }}</th>
          <th scope="col" class="num">{{ t('convert.dropped.count') }}</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="group in groups" :key="`${group.reason}:${group.class}`">
          <td>{{ group.class }}</td>
          <td>{{ t(`convert.dropped.reason_${group.reason}`) }}</td>
          <td class="num">{{ group.count }}</td>
        </tr>
      </tbody>
    </table>
    <template v-if="items.length > 0">
      <BaseButton variant="link" :aria-expanded="idsOpen" aria-controls="dropped-ids" @click="idsOpen = !idsOpen">
        {{ idsOpen ? t('convert.dropped.hideIds') : t('convert.dropped.showIds') }}
      </BaseButton>
      <ul v-show="idsOpen" id="dropped-ids" class="ids">
        <li v-for="item in shownItems" :key="item.elementId">
          {{ item.elementId }} · {{ item.class }} · {{ reasonOf(item) }}
        </li>
        <li v-if="items.length > SHOWN_IDS" class="more">
          {{ t('convert.dropped.truncated', { shown: SHOWN_IDS }) }}
        </li>
      </ul>
    </template>
  </section>
</template>

<style scoped>
.title {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: var(--space-1) var(--space-3);
  font-size: 1rem;
  margin: 0 0 var(--space-2);
}

.title :deep(.base-button) {
  font-size: 0.875rem;
}

table {
  width: 100%;
  border-collapse: collapse;
  font-size: 0.8125rem;
  margin-bottom: var(--space-2);
}

th,
td {
  text-align: left;
  padding: var(--space-1);
  border-bottom: 1px solid var(--color-border);
}

th {
  color: var(--color-text-muted);
  font-weight: 600;
}

.num {
  text-align: right;
}

.ids {
  list-style: none;
  margin: var(--space-2) 0 0;
  padding: var(--space-2);
  max-height: 7rem;
  overflow: auto;
  background: var(--color-surface);
  border-radius: var(--radius);
  font-family: var(--font-mono);
  font-size: 0.75rem;
  color: var(--color-text-muted);
}
</style>
