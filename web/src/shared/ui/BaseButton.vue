<script setup lang="ts">
withDefaults(defineProps<{ variant?: 'primary' | 'ghost' | 'link'; block?: boolean }>(), {
  variant: 'primary',
  block: false,
});
</script>

<template>
  <button type="button" class="base-button" :class="[`is-${variant}`, { 'is-block': block }]">
    <slot />
  </button>
</template>

<style scoped>
.base-button {
  font: inherit;
  font-weight: 600;
  cursor: pointer;
  border-radius: var(--radius);
  border: 1px solid transparent;
  padding: var(--space-3) var(--space-4);
  min-height: 2.5rem;
}

.is-primary {
  background: var(--color-primary);
  color: var(--color-primary-text);
}

.is-ghost {
  background: transparent;
  color: var(--color-primary);
  border-color: var(--color-primary);
}

.is-link {
  background: none;
  color: var(--color-primary);
  padding: 0;
  min-height: 0;
  font-weight: 500;
  text-decoration: underline;
}

.is-block {
  display: block;
  width: 100%;
}

.base-button:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}
</style>
