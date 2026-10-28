<script setup lang="ts">
import { useI18n } from 'vue-i18n';
import { RouterLink } from 'vue-router';

import { PAIRS, pairPath } from '../pairs';

const { t } = useI18n();
</script>

<template>
  <nav class="pairs" aria-labelledby="pairs-title">
    <h2 id="pairs-title">{{ t('convert.pairs.title') }}</h2>
    <ul>
      <li v-for="pair in PAIRS" :key="pairPath(pair)">
        <RouterLink :to="pairPath(pair)">{{ t('convert.pairs.link', { ...pair }) }}</RouterLink>
      </li>
    </ul>
  </nav>
</template>

<style scoped>
.pairs {
  width: min(40rem, 100%);
  margin-top: calc(var(--space-6) * 2);
}

h2 {
  font-size: 1.125rem;
  margin: 0 0 var(--space-3);
}

ul {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(11rem, 1fr));
  gap: var(--space-2) var(--space-4);
  list-style: none;
  margin: 0;
  padding: 0;
  font-size: 0.9375rem;
}
</style>
