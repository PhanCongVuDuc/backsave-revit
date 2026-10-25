<script setup lang="ts">
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';

import BaseButton from '@/shared/ui/BaseButton.vue';

const props = defineProps<{ code: string; retryAfterS: number | null; year: number | null }>();
const emit = defineEmits<{ chooseAnother: [] }>();
const i18n = useI18n();
const { t } = i18n;

const DEFAULT_RETRY_S = 3600;

const text = computed(() => {
  const key = i18n.te(`convert.error.${props.code}`) ? props.code : 'unknown';
  const minutes = Math.max(1, Math.ceil((props.retryAfterS ?? DEFAULT_RETRY_S) / 60));
  return t(`convert.error.${key}`, { year: props.year ?? '', minutes });
});
</script>

<template>
  <div class="error-panel">
    <p class="err" role="alert">{{ text }}</p>
    <BaseButton variant="ghost" @click="emit('chooseAnother')">{{ t('convert.chooseAnother') }}</BaseButton>
  </div>
</template>

<style scoped>
.error-panel {
  display: grid;
  gap: var(--space-3);
  justify-items: start;
}

.err {
  margin: 0;
  color: var(--color-danger);
  font-weight: 700;
}
</style>
