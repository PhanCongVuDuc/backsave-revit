<script setup lang="ts">
import { watchEffect } from 'vue';
import { useI18n } from 'vue-i18n';
import { RouterLink } from 'vue-router';

import { setPageMeta } from '@/shared/seo/pageMeta';

import { GUIDES, guidePath } from './guides';

const { t } = useI18n();

watchEffect(() => {
  setPageMeta({ title: t('guides.pageTitle'), description: t('guides.intro'), path: '/guides' });
});
</script>

<template>
  <article class="guides">
    <h1>{{ t('guides.title') }}</h1>
    <p class="intro">{{ t('guides.intro') }}</p>
    <ul>
      <li v-for="guide in GUIDES" :key="guide.slug">
        <RouterLink :to="guidePath(guide)">{{ t(`guides.articles.${guide.key}.title`) }}</RouterLink>
        <p>{{ t(`guides.articles.${guide.key}.description`) }}</p>
      </li>
    </ul>
  </article>
</template>

<style scoped>
.guides {
  max-width: 40rem;
  margin: 0 auto;
}

h1 {
  font-size: 1.75rem;
}

.intro {
  color: var(--color-text-muted);
}

ul {
  list-style: none;
  margin: var(--space-6) 0 0;
  padding: 0;
  display: grid;
  gap: var(--space-4);
}

li a {
  font-size: 1.0625rem;
  font-weight: 600;
}

li p {
  margin: var(--space-1) 0 0;
  color: var(--color-text-muted);
}
</style>
