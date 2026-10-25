<script setup lang="ts">
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';

import BaseButton from '@/shared/ui/BaseButton.vue';

import { formatClock } from '../format';

const props = defineProps<{
  mode: 'uploading' | 'interrupted' | 'queued' | 'running';
  /** Upload progress, 0 to 1. */
  progress: number;
  position: number | null;
  elapsedS: number;
  estimateS: number;
  from: number;
  to: number | null;
  /** The upload continues one this browser started earlier for the same file. */
  continuing?: boolean;
  /** This tab created the job; otherwise cancelling only leaves the page and deletes nothing. */
  owned?: boolean;
}>();
const emit = defineEmits<{ cancel: []; resume: [] }>();
const { t } = useI18n();

const percent = computed(() => {
  let ratio = props.progress;
  if (props.mode === 'running') {
    ratio = props.estimateS > 0 ? props.elapsedS / props.estimateS : 0;
  }
  return Math.min(100, Math.max(0, Math.round(ratio * 100)));
});

const text = computed(() => {
  switch (props.mode) {
    case 'uploading':
      return t('convert.progress.uploading', { percent: percent.value });
    case 'interrupted':
      return t('convert.progress.interrupted');
    case 'queued':
      return props.position === null
        ? t('convert.progress.waitingUnknown')
        : t('convert.progress.waiting', { n: props.position }, props.position);
    case 'running': {
      const params = { from: props.from, to: props.to ?? '?', elapsed: formatClock(props.elapsedS) };
      return props.estimateS > 0
        ? t('convert.progress.convertingEstimate', { ...params, estimate: formatClock(props.estimateS) })
        : t('convert.progress.converting', params);
    }
  }
});

const measurable = computed(() => props.mode !== 'queued');
</script>

<template>
  <div class="progress-panel">
    <p v-if="continuing && (mode === 'uploading' || mode === 'interrupted')" class="note">
      {{ t('convert.progress.continuing') }}
    </p>
    <p class="text">{{ text }}</p>
    <div
      class="bar"
      :class="{ 'is-waiting': !measurable }"
      role="progressbar"
      :aria-label="t('convert.progress.label')"
      aria-valuemin="0"
      aria-valuemax="100"
      :aria-valuenow="measurable ? percent : undefined"
      :aria-valuetext="text"
    >
      <i :style="{ width: measurable ? `${String(percent)}%` : '100%' }" />
    </div>
    <div class="actions">
      <BaseButton v-if="mode === 'interrupted'" @click="emit('resume')">{{ t('convert.resume') }}</BaseButton>
      <BaseButton variant="ghost" @click="emit('cancel')">
        {{ owned === false ? t('convert.leave') : t('convert.cancel') }}
      </BaseButton>
    </div>
  </div>
</template>

<style scoped>
.progress-panel {
  display: grid;
  gap: var(--space-3);
}

.text {
  margin: 0;
}

.note {
  margin: 0;
  color: var(--color-text-muted);
  font-size: 0.8125rem;
}

.bar {
  height: 0.5rem;
  background: var(--color-border);
  border-radius: 0.25rem;
  overflow: hidden;
}

.bar i {
  display: block;
  height: 100%;
  background: var(--color-primary);
  transition: width 0.2s;
}

.bar.is-waiting i {
  opacity: 0.35;
}

.actions {
  display: flex;
  gap: var(--space-3);
  flex-wrap: wrap;
}
</style>
