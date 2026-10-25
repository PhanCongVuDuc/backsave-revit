<script setup lang="ts">
export interface SelectOption {
  value: number | string;
  label: string;
}

defineProps<{ options: SelectOption[]; label: string }>();
const model = defineModel<number | string>({ required: true });
</script>

<template>
  <label class="base-select">
    <span class="visually-hidden">{{ label }}</span>
    <select v-model="model">
      <option v-for="option in options" :key="option.value" :value="option.value">{{ option.label }}</option>
    </select>
  </label>
</template>

<style scoped>
.base-select select {
  font: inherit;
  width: 100%;
  min-height: 2.5rem;
  padding: var(--space-2) var(--space-3);
  border: 1px solid var(--color-border);
  border-radius: var(--radius);
  background: var(--color-card);
  color: var(--color-text);
}
</style>
