<script setup lang="ts">
const model = defineModel<boolean>({ required: true });
</script>

<template>
  <label class="base-checkbox">
    <input v-model="model" type="checkbox" />
    <span><slot /></span>
  </label>
</template>

<style scoped>
.base-checkbox {
  display: flex;
  gap: var(--space-2);
  align-items: flex-start;
  font-size: 0.875rem;
}

.base-checkbox input {
  margin-top: 0.2rem;
  flex: none;
  width: 1rem;
  height: 1rem;
}
</style>
