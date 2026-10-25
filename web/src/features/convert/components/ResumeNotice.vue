<script setup lang="ts">
import { useI18n } from 'vue-i18n';

import BaseButton from '@/shared/ui/BaseButton.vue';

defineProps<{ name: string; mismatch: boolean }>();
const emit = defineEmits<{ startOver: [] }>();
const { t } = useI18n();
</script>

<template>
  <div class="resume-notice">
    <p :role="mismatch ? 'alert' : undefined">
      {{ mismatch ? t('convert.reopen.mismatch', { name }) : t('convert.reopen.prompt', { name }) }}
    </p>
    <BaseButton variant="ghost" @click="emit('startOver')">{{ t('convert.reopen.startOver') }}</BaseButton>
  </div>
</template>

<style scoped>
.resume-notice {
  display: grid;
  gap: var(--space-2);
  justify-items: start;
  margin-bottom: var(--space-3);
}

.resume-notice p {
  margin: 0;
  font-weight: 600;
}
</style>
