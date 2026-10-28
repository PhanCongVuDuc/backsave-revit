<script setup lang="ts">
import { watchEffect } from 'vue';
import { useI18n } from 'vue-i18n';
import { RouterLink } from 'vue-router';

import {
  AUTHOR_NAME,
  CONTACT_EMAIL,
  ISSUES_URL,
  LINKEDIN_URL,
  REPO_LABEL,
  REPO_URL as REPO,
} from '@/shared/contact/contact';
import { setPageMeta } from '@/shared/seo/pageMeta';

const ROWS = ['unfinished', 'cancelled', 'original', 'failed', 'output', 'report'] as const;

const { t } = useI18n();

watchEffect(() => {
  setPageMeta({ title: t('about.pageTitle'), description: t('about.intro'), path: '/how-it-works' });
});
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

    <h2>{{ t('about.contact.title') }}</h2>
    <i18n-t keypath="about.contact.body" tag="p" scope="global">
      <template #email>
        <a :href="`mailto:${CONTACT_EMAIL}`">{{ CONTACT_EMAIL }}</a>
      </template>
      <template #issues>
        <a :href="ISSUES_URL" rel="noopener">{{ t('about.contact.issues') }}</a>
      </template>
    </i18n-t>

    <h2>{{ t('about.source.title') }}</h2>
    <i18n-t keypath="about.openSource" tag="p" scope="global">
      <template #repo>
        <a :href="REPO" rel="noopener">{{ REPO_LABEL }}</a>
      </template>
    </i18n-t>
    <p>{{ t('about.source.body') }}</p>
    <ul>
      <li>
        <a :href="`${REPO}/blob/main/server/src/cleanup.rs`" rel="noopener">{{ t('about.source.cleanup') }}</a>
      </li>
      <li>
        <a :href="`${REPO}/blob/main/server/src/reports.rs`" rel="noopener">{{ t('about.source.reports') }}</a>
      </li>
    </ul>

    <i18n-t keypath="about.author" tag="p" scope="global" class="author">
      <template #name>{{ AUTHOR_NAME }}</template>
      <template #linkedin>
        <a :href="LINKEDIN_URL" rel="noopener">LinkedIn</a>
      </template>
    </i18n-t>

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

.author {
  color: var(--color-text-muted);
  font-size: 0.875rem;
}
</style>
