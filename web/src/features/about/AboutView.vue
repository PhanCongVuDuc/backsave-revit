<script setup lang="ts">
import { useI18n } from 'vue-i18n';
import { RouterLink } from 'vue-router';

const REPO = 'https://github.com/PhanCongVuDuc/backsave-revit';
const ROWS = ['unfinished', 'original', 'failed', 'output', 'report'] as const;

const { t } = useI18n();
</script>

<template>
  <article class="about">
    <h1>{{ t('about.title') }}</h1>
    <p>{{ t('about.intro') }}</p>

    <h2>{{ t('about.sent.title') }}</h2>
    <p>{{ t('about.sent.body') }}</p>

    <h2>{{ t('about.deletion.title') }}</h2>
    <table>
      <thead>
        <tr>
          <th scope="col">{{ t('about.deletion.what') }}</th>
          <th scope="col">{{ t('about.deletion.when') }}</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="row in ROWS" :key="row">
          <td>{{ t(`about.deletion.rows.${row}.what`) }}</td>
          <td>{{ t(`about.deletion.rows.${row}.when`) }}</td>
        </tr>
      </tbody>
    </table>

    <h2>{{ t('about.cookies.title') }}</h2>
    <p>{{ t('about.cookies.body') }}</p>

    <h2>{{ t('about.logs.title') }}</h2>
    <p>{{ t('about.logs.body') }}</p>

    <h2>{{ t('about.source.title') }}</h2>
    <p>{{ t('about.source.body') }}</p>
    <ul>
      <li>
        <a :href="`${REPO}/blob/main/server/src/cleanup.rs`" rel="noopener">{{ t('about.source.cleanup') }}</a>
      </li>
      <li>
        <a :href="`${REPO}/blob/main/server/src/reports.rs`" rel="noopener">{{ t('about.source.reports') }}</a>
      </li>
    </ul>

    <p>
      <RouterLink :to="{ name: 'convert' }">{{ t('about.back') }}</RouterLink>
    </p>
  </article>
</template>

<style scoped>
.about {
  max-width: 40rem;
  margin: 0 auto;
}

h1 {
  font-size: 1.75rem;
}

h2 {
  font-size: 1.125rem;
  margin-top: var(--space-6);
}

table {
  width: 100%;
  border-collapse: collapse;
  font-size: 0.9375rem;
}

th,
td {
  text-align: left;
  vertical-align: top;
  padding: var(--space-2);
  border-bottom: 1px solid var(--color-border);
}

th {
  color: var(--color-text-muted);
}
</style>
