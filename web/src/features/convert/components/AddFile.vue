<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, useTemplateRef } from 'vue';
import { useI18n } from 'vue-i18n';

const emit = defineEmits<{ file: [file: File] }>();
const { t } = useI18n();
const input = useTemplateRef<HTMLInputElement>('input');
const over = ref(false);
// dragenter/dragleave fire for every child element, so the zone counts how many it is inside.
let depth = 0;

// A file dropped just outside the zone would make the browser open it; only the zone takes drops.
function ignoreStray(event: DragEvent) {
  event.preventDefault();
}

onMounted(() => {
  window.addEventListener('dragover', ignoreStray);
  window.addEventListener('drop', ignoreStray);
});
onBeforeUnmount(() => {
  window.removeEventListener('dragover', ignoreStray);
  window.removeEventListener('drop', ignoreStray);
});

function enter() {
  depth += 1;
  over.value = true;
}

function leave() {
  depth = Math.max(0, depth - 1);
  over.value = depth > 0;
}

function choose(file: File | undefined) {
  if (file) {
    emit('file', file);
  }
}

function onChange(event: Event) {
  const element = event.target as HTMLInputElement;
  choose(element.files?.[0]);
  // The same file can then be chosen again after "Choose another file".
  element.value = '';
}

function onDrop(event: DragEvent) {
  depth = 0;
  over.value = false;
  choose(event.dataTransfer?.files[0]);
}
</script>

<template>
  <div
    class="add-file"
    :class="{ 'is-over': over }"
    data-testid="drop-zone"
    @dragenter.prevent="enter"
    @dragover.prevent
    @dragleave="leave"
    @drop.prevent="onDrop"
  >
    <button type="button" class="plus" @click="input?.click()">
      <span class="plus-sign" aria-hidden="true">+</span>
      <span class="plus-label">{{ t('convert.add.button') }}</span>
    </button>
    <p class="drop-hint">{{ t('convert.add.drop') }}</p>
    <p class="limits">{{ t('convert.add.limits') }}</p>
    <input
      ref="input"
      type="file"
      hidden
      accept=".rvt,.rte,.rfa,.rft"
      :aria-label="t('convert.add.input')"
      @change="onChange"
    />
  </div>
</template>

<style scoped>
.add-file {
  border: 2px dashed var(--color-border);
  border-radius: 0.75rem;
  padding: var(--space-6) var(--space-3);
  text-align: center;
  transition:
    background 0.15s,
    border-color 0.15s;
}

.add-file.is-over {
  background: var(--color-primary-soft);
  border-color: var(--color-primary);
}

.plus {
  display: inline-flex;
  flex-direction: column;
  align-items: center;
  gap: var(--space-2);
  background: none;
  border: 0;
  color: var(--color-text);
  font: inherit;
  font-weight: 600;
  cursor: pointer;
  padding: var(--space-2);
  border-radius: var(--radius);
}

.plus-sign {
  width: 3rem;
  height: 3rem;
  border-radius: 50%;
  background: var(--color-primary);
  color: var(--color-primary-text);
  font-size: 1.75rem;
  line-height: 3rem;
}

.drop-hint,
.limits {
  margin: var(--space-1) 0 0;
  color: var(--color-text-muted);
  font-size: 0.875rem;
}

.limits {
  font-size: 0.8125rem;
}
</style>
