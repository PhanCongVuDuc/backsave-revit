<script setup lang="ts">
import { useI18n } from 'vue-i18n';

import BaseButton from '@/shared/ui/BaseButton.vue';

import TargetPicker from './TargetPicker.vue';

defineProps<{
  name: string;
  summary: string;
  targets: number[];
  /** The button continues an upload this browser started earlier for the same file. */
  continuing?: boolean;
}>();
const emit = defineEmits<{ downgrade: []; chooseAnother: [] }>();
const target = defineModel<number>({ required: true });
const { t } = useI18n();
</script>

<template>
  <div class="file-summary">
    <div class="file">
      <span class="file-name">{{ name }}</span>
      <span class="file-meta">{{ summary }}</span>
    </div>
    <div class="row">
      <span class="arrow" aria-hidden="true">→</span>
      <TargetPicker v-model="target" :targets="targets" />
    </div>
    <p v-if="continuing" class="note">{{ t('convert.progress.continuing') }}</p>
    <BaseButton block @click="emit('downgrade')">
      {{ continuing ? t('convert.continueUpload') : t('convert.downgrade') }}
    </BaseButton>
    <p class="privacy">{{ continuing ? t('convert.privacyContinue') : t('convert.privacy') }}</p>
    <BaseButton variant="link" @click="emit('chooseAnother')">{{ t('convert.chooseAnother') }}</BaseButton>
  </div>
</template>

<style scoped>
.file-summary {
  display: grid;
  gap: var(--space-3);
  justify-items: start;
}

.file {
  display: grid;
  gap: var(--space-1);
  width: 100%;
  background: var(--color-surface);
  border-radius: var(--radius);
  padding: var(--space-3);
}

.note {
  margin: 0;
  color: var(--color-text-muted);
  font-size: 0.8125rem;
}

.file-name {
  font-weight: 600;
  overflow-wrap: anywhere;
}

.file-meta {
  color: var(--color-text-muted);
  font-size: 0.875rem;
}

.row {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  width: 100%;
}

.row :deep(.base-select) {
  flex: 1;
}

.file-summary :deep(.is-block) {
  width: 100%;
}

.privacy {
  margin: 0;
  color: var(--color-text-muted);
  font-size: 0.8125rem;
}
</style>
