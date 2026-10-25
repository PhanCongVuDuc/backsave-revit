<script setup lang="ts">
import { useI18n } from 'vue-i18n';

import BaseButton from '@/shared/ui/BaseButton.vue';

import type { DroppedItem } from '../dropped';
import { formatBytes, formatTimeOfDay } from '../format';
import DroppedTable from './DroppedTable.vue';

defineProps<{
  fileName: string;
  size: number;
  downloadUrl: string;
  /** The Revit version the file was converted to, when known. */
  target: number | null;
  dropped: DroppedItem[] | null;
  droppedError: string | null;
  expiresAt: string | null;
}>();
const emit = defineEmits<{ deleteNow: []; convertAnother: [] }>();
const { t, locale } = useI18n();
</script>

<template>
  <div class="result-panel">
    <p class="ok">
      {{ target === null ? t('convert.result.titleReady') : t('convert.result.title', { year: target }) }}
    </p>
    <a class="download" :href="downloadUrl" :download="fileName">
      {{ t('convert.result.download', { file: fileName, size: formatBytes(size) }) }}
    </a>
    <DroppedTable v-if="dropped" :items="dropped" />
    <p v-else-if="droppedError" class="muted" role="status">{{ t('convert.dropped.loadFailed') }}</p>
    <p v-else class="muted">{{ t('convert.dropped.loading') }}</p>
    <p class="muted footer">
      <span v-if="expiresAt">{{ t('convert.result.expires', { time: formatTimeOfDay(expiresAt, locale) }) }}</span>
      <BaseButton variant="link" @click="emit('deleteNow')">{{ t('convert.result.deleteNow') }}</BaseButton>
      <BaseButton variant="link" @click="emit('convertAnother')">{{ t('convert.convertAnother') }}</BaseButton>
    </p>
  </div>
</template>

<style scoped>
.result-panel {
  display: grid;
  gap: var(--space-3);
}

.ok {
  margin: 0;
  color: var(--color-ok);
  font-weight: 700;
}

.download {
  display: block;
  text-align: center;
  background: var(--color-primary);
  color: var(--color-primary-text);
  font-weight: 600;
  text-decoration: none;
  border-radius: var(--radius);
  padding: var(--space-3) var(--space-4);
  overflow-wrap: anywhere;
}

.muted {
  margin: 0;
  color: var(--color-text-muted);
  font-size: 0.8125rem;
}

.footer {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-1) var(--space-3);
  align-items: baseline;
}
</style>
