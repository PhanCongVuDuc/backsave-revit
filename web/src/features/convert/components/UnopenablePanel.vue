<script setup lang="ts">
import { computed, ref, shallowRef } from 'vue';
import { useI18n } from 'vue-i18n';

import BaseButton from '@/shared/ui/BaseButton.vue';
import BaseCheckbox from '@/shared/ui/BaseCheckbox.vue';

const props = defineProps<{
  reportCode: string | null;
  /** The tab still holds the original file, so the user does not have to choose it again. */
  canReuseFile: boolean;
  sending: boolean;
  /** Upload progress of the original, 0 to 1. */
  progress: number;
  /** Why the last attempt failed, if it did. */
  error: string | null;
}>();
const emit = defineEmits<{ send: [note: string, file: File | undefined] }>();
const NOTE_MAX = 500;
const i18n = useI18n();
const { t } = i18n;
// Never ticked for the user: the files are kept only after an explicit choice.
const consent = ref(false);
const note = ref('');
const picked = shallowRef<File | null>(null);

/** Expired or deleted: nothing can be sent from here any more. */
const gone = computed(() => props.error === 'expired' || props.error === 'not_found');
const needsFile = computed(() => !props.canReuseFile);
const canSend = computed(() => consent.value && !props.sending && (!needsFile.value || picked.value !== null));
const failure = computed(() => {
  if (props.error === null) {
    return null;
  }
  const key = `convert.unopenable.failed_${props.error}`;
  return i18n.te(key) ? t(key) : t('convert.unopenable.failed', { code: props.error });
});

function onPick(event: Event) {
  picked.value = (event.target as HTMLInputElement).files?.[0] ?? null;
}

function send() {
  if (canSend.value) {
    emit('send', note.value, needsFile.value ? (picked.value ?? undefined) : undefined);
  }
}
</script>

<template>
  <div class="unopenable-panel">
    <p v-if="reportCode" class="muted" role="status">{{ t('convert.unopenable.sent', { code: reportCode }) }}</p>
    <p v-else-if="gone" class="err" role="alert">{{ failure }}</p>
    <form v-else class="report" @submit.prevent="send">
      <p class="intro">{{ t('convert.unopenable.intro') }}</p>
      <BaseCheckbox v-model="consent">{{ t('convert.unopenable.consent') }}</BaseCheckbox>
      <label class="field">
        <span>{{ t('convert.unopenable.note') }}</span>
        <textarea v-model="note" :maxlength="NOTE_MAX" rows="2"></textarea>
      </label>
      <label v-if="needsFile" class="field">
        <span>{{ t('convert.unopenable.pick') }}</span>
        <input type="file" accept=".rvt,.rte,.rfa,.rft" @change="onPick" />
      </label>
      <BaseButton type="submit" :disabled="!canSend">
        {{
          sending
            ? t('convert.unopenable.sending', { percent: Math.round(progress * 100) })
            : t('convert.unopenable.send')
        }}
      </BaseButton>
      <p v-if="failure" class="err small" role="alert">{{ failure }}</p>
    </form>
  </div>
</template>

<style scoped>
.unopenable-panel {
  display: grid;
  gap: var(--space-3);
  border-top: 1px solid var(--color-border);
  padding-top: var(--space-3);
}

.report {
  display: grid;
  gap: var(--space-3);
  justify-items: start;
}

.intro {
  margin: 0;
  font-size: 0.875rem;
}

.field {
  display: grid;
  gap: var(--space-1);
  width: 100%;
  font-size: 0.875rem;
}

.field textarea {
  font: inherit;
  width: 100%;
  box-sizing: border-box;
  resize: vertical;
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
</style>
