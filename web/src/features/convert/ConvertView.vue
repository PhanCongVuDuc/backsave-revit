<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute, useRouter } from 'vue-router';

import AddFile from './components/AddFile.vue';
import ErrorPanel from './components/ErrorPanel.vue';
import FailurePanel from './components/FailurePanel.vue';
import FileSummary from './components/FileSummary.vue';
import ProgressPanel from './components/ProgressPanel.vue';
import ResultPanel from './components/ResultPanel.vue';
import { formatBytes } from './format';
import { useConvertStore } from './stores/convertStore';

const { t } = useI18n();
const store = useConvertStore();
const route = useRoute();
const router = useRouter();
const sending = ref(false);

const summary = computed(() =>
  store.info && store.file
    ? t('convert.summary', {
        year: store.info.year,
        kind: t(`convert.kind.${store.info.kind}`),
        size: formatBytes(store.file.size),
      })
    : '',
);

const interrupted = computed(() => store.phase === 'error' && store.error?.code === 'upload_failed');
const progressMode = computed(() => {
  if (interrupted.value) {
    return 'interrupted';
  }
  return store.phase === 'queued' || store.phase === 'running' ? store.phase : 'uploading';
});
const showProgress = computed(
  () => interrupted.value || store.phase === 'uploading' || store.phase === 'queued' || store.phase === 'running',
);
const showError = computed(() => store.phase === 'error' && !interrupted.value);

/** Read out by screen readers whenever the card moves to another step. */
const announcement = computed(() => t(`convert.status.${interrupted.value ? 'uploading' : store.phase}`));

const targetModel = computed({
  get: () => store.target ?? 0,
  set: (year: number) => {
    store.setTarget(year);
  },
});

async function send() {
  sending.value = true;
  try {
    await store.consentReport();
  } finally {
    sending.value = false;
  }
}

// The address follows the job: /job/<id> while there is one, / otherwise.
watch(
  () => store.jobId,
  (id) => {
    if (id && route.name === 'convert') {
      void router.replace({ name: 'job', params: { id } });
    } else if (!id && route.name === 'job') {
      void router.replace({ name: 'convert' });
    }
  },
);

watch(
  () => route.params.id,
  (id) => {
    if (route.name === 'job' && typeof id === 'string' && id !== store.jobId) {
      void follow(id);
    }
  },
);

async function follow(id?: string) {
  const found = await store.resume(id);
  if (!found && route.name === 'job') {
    await router.replace({ name: 'convert' });
  }
}

onMounted(() => {
  const routeId = route.name === 'job' && typeof route.params.id === 'string' ? route.params.id : undefined;
  if (routeId) {
    if (routeId !== store.jobId) {
      void follow(routeId);
    }
  } else if (store.jobId) {
    void router.replace({ name: 'job', params: { id: store.jobId } });
  } else {
    void follow();
  }
});
</script>

<template>
  <section class="convert">
    <h1>{{ t('convert.title') }}</h1>
    <p class="lead">{{ t('convert.lead') }}</p>
    <div class="card">
      <AddFile v-if="store.phase === 'pick'" @file="store.chooseFile" />
      <FileSummary
        v-else-if="store.phase === 'ready' && store.file"
        v-model="targetModel"
        :name="store.file.name"
        :summary="summary"
        :targets="store.targets"
        @downgrade="store.submit"
        @choose-another="store.cancel"
      />
      <ProgressPanel
        v-else-if="showProgress"
        :mode="progressMode"
        :progress="store.progress"
        :position="store.position"
        :elapsed-s="store.elapsedS"
        :estimate-s="store.estimateS"
        :from="store.info?.year ?? 0"
        :to="store.target"
        @cancel="store.cancel"
        @resume="store.retryUpload"
      />
      <ResultPanel
        v-else-if="store.phase === 'done' && store.result && store.downloadUrl"
        :file-name="store.result.fileName"
        :size="store.result.size"
        :download-url="store.downloadUrl"
        :target="store.target"
        :dropped="store.dropped"
        :dropped-error="store.droppedError"
        :expires-at="store.expiresAt"
        @delete-now="store.deleteNow"
        @convert-another="store.cancel"
      />
      <FailurePanel
        v-else-if="store.phase === 'failed' && store.error"
        :kind="store.error.code"
        :message="store.error.message"
        :report-code="store.reportCode"
        :report-error="store.reportError"
        :sending="sending"
        @send="send"
        @choose-another="store.cancel"
      />
      <ErrorPanel
        v-else-if="showError && store.error"
        :code="store.error.code"
        :retry-after-s="store.error.retryAfterS"
        :year="store.info?.year ?? null"
        @choose-another="store.cancel"
      />
    </div>
    <p class="visually-hidden" role="status" aria-live="polite">{{ announcement }}</p>
  </section>
</template>

<style scoped>
.convert {
  display: flex;
  flex-direction: column;
  align-items: center;
}

h1 {
  font-size: 1.75rem;
  margin: var(--space-6) 0 var(--space-1);
  text-align: center;
}

.lead {
  color: var(--color-text-muted);
  margin: 0 0 var(--space-6);
  text-align: center;
}

.card {
  width: min(27.5rem, 100%);
  background: var(--color-card);
  border-radius: 1.125rem;
  box-shadow: var(--shadow-card);
  padding: var(--space-6);
}
</style>
