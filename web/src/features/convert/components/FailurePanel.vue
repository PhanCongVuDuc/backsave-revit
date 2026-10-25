<script setup lang="ts">
import { computed, ref } from 'vue';
import { useI18n } from 'vue-i18n';

import BaseButton from '@/shared/ui/BaseButton.vue';
import BaseCheckbox from '@/shared/ui/BaseCheckbox.vue';

const props = defineProps<{
  /** The job's error kind. */
  kind: string;
  message: string | null;
  reportCode: string | null;
  /** Why the last report request failed, if it did. */
  reportError: string | null;
  sending: boolean;
}>();
const emit = defineEmits<{ send: []; chooseAnother: [] }>();
const i18n = useI18n();
const { t } = i18n;
// Never ticked for the user: the file is kept only after an explicit choice.
const consent = ref(false);

const failure = computed(() => {
  const key = i18n.te(`convert.failure.${props.kind}`) ? props.kind : 'unknown';
  return t(`convert.failure.${key}`, { message: props.message ?? '' });
});

const reportFailure = computed(() => {
  if (props.reportError === null) {
    return null;
  }
  const key = `convert.report.failed_${props.reportError}`;
  return i18n.te(key) ? t(key) : t('convert.report.failed', { code: props.reportError });
});
</script>

<template>
  <div class="failure-panel">
    <p class="err" role="alert">{{ failure }}</p>
    <p v-if="reportCode" class="muted">{{ t('convert.report.sent', { code: reportCode }) }}</p>
    <form v-else class="report" @submit.prevent="emit('send')">
      <BaseCheckbox v-model="consent">{{ t('convert.report.consent') }}</BaseCheckbox>
      <BaseButton type="submit" :disabled="!consent || sending">{{ t('convert.report.send') }}</BaseButton>
      <p v-if="reportFailure" class="err small" role="alert">{{ reportFailure }}</p>
    </form>
    <BaseButton variant="ghost" @click="emit('chooseAnother')">{{ t('convert.chooseAnother') }}</BaseButton>
  </div>
</template>

<style scoped>
.failure-panel {
  display: grid;
  gap: var(--space-3);
  justify-items: start;
}

.err {
  margin: 0;
  color: var(--color-danger);
  font-weight: 700;
}

.small {
  font-weight: 500;
  font-size: 0.875rem;
}

.muted {
  margin: 0;
  color: var(--color-text-muted);
  font-size: 0.875rem;
}

.report {
  display: grid;
  gap: var(--space-3);
  justify-items: start;
}
</style>
