<script setup lang="ts">
import { useI18n } from 'vue-i18n';

defineProps<{ targets: number[] }>();
const model = defineModel<number>({ required: true });
const { t } = useI18n();
</script>

<template>
  <fieldset class="target-picker">
    <legend class="visually-hidden">{{ t('convert.target.label') }}</legend>
    <label v-for="year in targets" :key="year" class="year" :class="{ 'is-on': model === year }">
      <input v-model="model" class="visually-hidden" type="radio" name="target-year" :value="year" />
      {{ t('convert.target.option', { year }) }}
    </label>
  </fieldset>
</template>

<style scoped>
.target-picker {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
  border: 0;
  margin: 0;
  padding: 0;
}

.year {
  cursor: pointer;
  padding: var(--space-2) var(--space-4);
  border: 1px solid var(--color-border);
  border-radius: 999px;
  background: var(--color-card);
  font-weight: 600;
}

.year:hover {
  border-color: var(--color-primary);
}

.year.is-on {
  background: var(--color-primary);
  border-color: var(--color-primary);
  color: var(--color-primary-text);
}

.year:has(input:focus-visible) {
  outline: 2px solid var(--color-focus);
  outline-offset: 2px;
}
</style>
