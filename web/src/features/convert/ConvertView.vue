<script setup lang="ts">
import { computed, nextTick, onMounted, ref, useTemplateRef, watch, watchEffect } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute, useRouter } from 'vue-router';

import { setPageMeta } from '@/shared/seo/pageMeta';

import AddFile from './components/AddFile.vue';
import ErrorPanel from './components/ErrorPanel.vue';
import FailurePanel from './components/FailurePanel.vue';
import FaqSection from './components/FaqSection.vue';
import FileSummary from './components/FileSummary.vue';
import GuideSections from './components/GuideSections.vue';
import PairLinks from './components/PairLinks.vue';
import ProgressPanel from './components/ProgressPanel.vue';
import ResultPanel from './components/ResultPanel.vue';
import UsageStats from './components/UsageStats.vue';
import { formatBytes } from './format';
import { pairPath, type Pair } from './pairs';
import { useConvertStore } from './stores/convertStore';

const { t } = useI18n();
const store = useConvertStore();
const route = useRoute();
const router = useRouter();
const sending = ref(false);
const card = useTemplateRef<HTMLElement>('card');

/** On a pair's page (/revit-2026-to-2024) the heading names it and its target is picked when the file allows. */
const pair = computed<Pair | null>(() =>
  route.name === 'pair' ? { from: Number(route.params.from), to: Number(route.params.to) } : null,
);
const heading = computed(() => (pair.value ? t('convert.pairPage.title', { ...pair.value }) : t('convert.title')));

watchEffect(() => {
  if (route.name === 'job') {
    return;
  }
  setPageMeta(
    pair.value
      ? {
          title: t('convert.pairPage.pageTitle', { ...pair.value }),
          description: t('convert.pairPage.description', { ...pair.value }),
          path: pairPath(pair.value),
        }
      : { title: t('convert.pageTitle'), description: t('convert.description'), path: '/' },
  );
});

watch(
  () => [store.phase, pair.value?.to] as const,
  ([phase, to]) => {
    if (phase === 'ready' && to) {
      store.setTarget(to);
    }
  },
  { immediate: true },
);

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

// After a step change the focused control is gone; move focus to the new step's first control.
watch(
  () => [store.phase, interrupted.value],
  async () => {
    await nextTick();
    const target =
      card.value?.querySelector<HTMLElement>('input:checked') ??
      card.value?.querySelector<HTMLElement>('button, a[href], select, input:not([hidden])');
    (target ?? card.value)?.focus();
  },
);

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
    if (id && (route.name === 'convert' || route.name === 'pair')) {
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
    <h1>{{ heading }}</h1>
    <p class="lead">{{ t('convert.lead') }}</p>
    <p class="versions">{{ t('convert.versions') }}</p>
    <div ref="card" class="card" tabindex="-1">
      <AddFile v-if="store.phase === 'pick'" @file="store.chooseFile" />
      <FileSummary
        v-else-if="store.phase === 'ready' && store.file"
        v-model="targetModel"
        :name="store.file.name"
        :summary="summary"
        :targets="store.targets"
        :continuing="store.continuing"
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
        :continuing="store.continuing"
        :owned="store.ownsJob"
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
        :report-code="store.reportCode"
        :can-reuse-file="store.canReuseFile"
        :report-sending="store.unopenableSending"
        :report-progress="store.unopenableProgress"
        :report-error="store.unopenableError"
        @delete-now="store.deleteNow"
        @convert-another="store.cancel"
        @report-unopenable="store.reportUnopenable"
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
    <!-- Outside the steps, so it is fetched once per page load and stays put while a file converts. -->
    <UsageStats />
    <template v-if="store.phase === 'pick'">
      <GuideSections />
      <FaqSection />
      <PairLinks />
    </template>
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
  margin: 0 0 var(--space-1);
  text-align: center;
}

.versions {
  color: var(--color-text-muted);
  font-size: 0.9375rem;
  margin: 0 0 var(--space-6);
  text-align: center;
}

.card:focus {
  outline: none;
}

.card {
  width: min(27.5rem, 100%);
  background: var(--color-card);
  border-radius: 1.125rem;
  box-shadow: var(--shadow-card);
  padding: var(--space-6);
}
</style>
