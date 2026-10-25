<script setup lang="ts">
import { useI18n } from 'vue-i18n';
import { RouterLink, RouterView } from 'vue-router';

import { LOCALES } from '@/app/i18n';

const { t, locale } = useI18n();
</script>

<template>
  <div class="shell">
    <header class="app-header">
      <RouterLink class="wordmark" :to="{ name: 'convert' }" :aria-label="t('nav.home')">
        Backsave <span class="light">for Revit</span>
      </RouterLink>
      <nav class="links">
        <RouterLink :to="{ name: 'about' }">{{ t('nav.about') }}</RouterLink>
        <a href="https://github.com/PhanCongVuDuc/backsave-revit" rel="noopener">{{ t('nav.source') }}</a>
        <span class="languages" role="group" :aria-label="t('app.language')">
          <button
            v-for="code in LOCALES"
            :key="code"
            type="button"
            :aria-pressed="locale === code"
            :class="{ 'is-on': locale === code }"
            @click="locale = code"
          >
            {{ code.toUpperCase() }}
          </button>
        </span>
      </nav>
    </header>
    <main>
      <RouterView />
    </main>
    <footer class="app-footer">{{ t('footer.trademark') }}</footer>
  </div>
</template>

<style scoped>
.shell {
  min-height: 100vh;
  display: flex;
  flex-direction: column;
}

.app-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  flex-wrap: wrap;
  gap: var(--space-2) var(--space-4);
  padding: var(--space-3) var(--space-4);
  font-size: 0.875rem;
}

.wordmark {
  color: var(--color-text);
  font-size: 1.0625rem;
  font-weight: 700;
  text-decoration: none;
}

.light {
  font-weight: 400;
  color: var(--color-text-muted);
}

.links {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: var(--space-2) var(--space-4);
}

.links a {
  color: var(--color-text-muted);
}

.languages {
  display: inline-flex;
  border: 1px solid var(--color-border);
}

.languages button {
  font: inherit;
  font-size: 0.8125rem;
  padding: var(--space-1) var(--space-2);
  background: transparent;
  color: var(--color-text-muted);
  border: 0;
  cursor: pointer;
}

.languages button:first-child {
  border-radius: var(--radius) 0 0 var(--radius);
}

.languages button:last-child {
  border-radius: 0 var(--radius) var(--radius) 0;
}

.languages button.is-on {
  background: var(--color-primary);
  color: var(--color-primary-text);
}

main {
  flex: 1;
  width: 100%;
  max-width: 48rem;
  margin: 0 auto;
  padding: var(--space-4);
}

.app-footer {
  text-align: center;
  color: var(--color-text-muted);
  font-size: 0.75rem;
  padding: var(--space-3) var(--space-4) var(--space-4);
}
</style>
