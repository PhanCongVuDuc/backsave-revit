<script setup lang="ts">
import { useI18n } from 'vue-i18n';

const { t } = useI18n();

const STEPS = ['add', 'pick', 'download'] as const;
const HIGHLIGHTS = ['free', 'noInstall', 'deleted', 'openSource'] as const;
</script>

<template>
  <section class="guide" aria-labelledby="steps-title">
    <h2 id="steps-title">{{ t('convert.guide.stepsTitle') }}</h2>
    <ol class="steps">
      <li v-for="(key, i) in STEPS" :key="key">
        <span class="number" aria-hidden="true">{{ i + 1 }}</span>
        <strong>{{ t(`convert.guide.steps.${key}.title`) }}</strong>
        <span>{{ t(`convert.guide.steps.${key}.body`) }}</span>
      </li>
    </ol>
  </section>

  <section class="guide" aria-labelledby="highlights-title">
    <h2 id="highlights-title" class="visually-hidden">{{ t('convert.guide.highlightsTitle') }}</h2>
    <ul class="highlights">
      <li v-for="key in HIGHLIGHTS" :key="key">
        <span class="check" aria-hidden="true">✓</span>
        <strong>{{ t(`convert.guide.highlights.${key}.title`) }}</strong>
        <span>{{ t(`convert.guide.highlights.${key}.body`) }}</span>
      </li>
    </ul>
  </section>
</template>

<style scoped>
.guide {
  width: min(40rem, 100%);
  margin-top: calc(var(--space-6) * 2);
}

h2 {
  font-size: 1.125rem;
  margin: 0 0 var(--space-4);
  text-align: center;
}

.steps,
.highlights {
  display: grid;
  gap: var(--space-4);
  list-style: none;
  margin: 0;
  padding: 0;
}

.steps {
  grid-template-columns: repeat(auto-fit, minmax(10rem, 1fr));
}

.highlights {
  grid-template-columns: repeat(auto-fit, minmax(8.5rem, 1fr));
}

li {
  display: grid;
  gap: var(--space-1);
  justify-items: center;
  text-align: center;
  font-size: 0.9375rem;
}

li span:last-child {
  color: var(--color-text-muted);
}

.number,
.check {
  display: grid;
  place-items: center;
  width: 2.25rem;
  height: 2.25rem;
  border-radius: 50%;
  background: var(--color-primary-soft);
  color: var(--color-primary);
  font-weight: 700;
}
</style>
