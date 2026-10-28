<script setup lang="ts">
import { computed, watchEffect } from 'vue';
import { useI18n } from 'vue-i18n';
import { RouterLink, useRoute } from 'vue-router';

import { setPageMeta } from '@/shared/seo/pageMeta';

import { GUIDES, guideBySlug, guidePath } from './guides';

interface Section {
  heading: string;
  paragraphs: string[];
}

const i18n = useI18n();
const { t } = i18n;
const route = useRoute();

// The router only opens this view for a known slug.
const guide = computed(() => guideBySlug(String(route.params.slug)) ?? GUIDES[0]);
const others = computed(() => GUIDES.filter((g) => g.slug !== guide.value.slug));
const sections = computed(() => {
  const raw: unknown = i18n.tm(`guides.articles.${guide.value.key}.sections`);
  return (raw as Section[]).map((s) => ({
    heading: i18n.rt(s.heading),
    paragraphs: s.paragraphs.map((p) => i18n.rt(p)),
  }));
});

watchEffect(() => {
  setPageMeta({
    title: `${t(`guides.articles.${guide.value.key}.title`)} | Revit Downgrade`,
    description: t(`guides.articles.${guide.value.key}.description`),
    path: guidePath(guide.value),
  });
});
</script>

<template>
  <article class="guide">
    <p class="crumb">
      <RouterLink :to="{ name: 'guides' }">{{ t('guides.all') }}</RouterLink>
    </p>
    <h1>{{ t(`guides.articles.${guide.key}.title`) }}</h1>
    <p class="intro">{{ t(`guides.articles.${guide.key}.description`) }}</p>
    <section v-for="section in sections" :key="section.heading">
      <h2>{{ section.heading }}</h2>
      <p v-for="paragraph in section.paragraphs" :key="paragraph">{{ paragraph }}</p>
    </section>
    <RouterLink class="cta" :to="{ name: 'convert' }">{{ t('guides.cta') }}</RouterLink>
    <nav class="others" :aria-label="t('guides.others')">
      <h2>{{ t('guides.others') }}</h2>
      <ul>
        <li v-for="other in others" :key="other.slug">
          <RouterLink :to="guidePath(other)">{{ t(`guides.articles.${other.key}.title`) }}</RouterLink>
        </li>
      </ul>
    </nav>
  </article>
</template>

<style scoped>
.guide {
  max-width: 40rem;
  margin: 0 auto;
}

.crumb {
  margin: 0;
  font-size: 0.875rem;
}

h1 {
  font-size: 1.75rem;
}

.intro {
  color: var(--color-text-muted);
  font-size: 1.0625rem;
}

h2 {
  font-size: 1.125rem;
  margin-top: var(--space-6);
}

.cta {
  display: inline-block;
  margin-top: var(--space-6);
  padding: var(--space-3) var(--space-6);
  border-radius: var(--radius);
  background: var(--color-primary);
  color: var(--color-primary-text);
  font-weight: 600;
  text-decoration: none;
}

.others ul {
  padding-left: var(--space-6);
}
</style>
