<script setup lang="ts">
import { useI18n } from 'vue-i18n';
import { RouterLink, RouterView } from 'vue-router';

const { t } = useI18n();
</script>

<template>
  <header class="app-header">
    <span class="app-name">{{ t('app.name') }}</span>
    <nav>
      <RouterLink :to="{ name: 'convert' }">{{ t('nav.convert') }}</RouterLink>
      <RouterLink :to="{ name: 'about' }">{{ t('nav.about') }}</RouterLink>
    </nav>
  </header>
  <main>
    <RouterView />
  </main>
</template>

<style scoped>
.app-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-4);
  padding: var(--space-3) var(--space-4);
  border-bottom: 1px solid var(--color-border);
}

.app-name {
  font-weight: 600;
}

nav {
  display: flex;
  gap: var(--space-4);
}

main {
  max-width: 48rem;
  margin: 0 auto;
  padding: var(--space-4);
}
</style>
