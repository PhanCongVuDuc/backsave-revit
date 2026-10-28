<script setup lang="ts">
import { useI18n } from 'vue-i18n';

const { t } = useI18n();

// Phrased the way people search for this, so the answers are also what search engines index.
const QUESTIONS = ['howTo', 'saveOlder', 'supported', 'lost', 'safe'] as const;
</script>

<template>
  <section class="faq" aria-labelledby="faq-title">
    <h2 id="faq-title">{{ t('convert.faq.title') }}</h2>
    <details v-for="key in QUESTIONS" :key="key">
      <summary>{{ t(`convert.faq.items.${key}.q`) }}</summary>
      <p>{{ t(`convert.faq.items.${key}.a`) }}</p>
    </details>
  </section>
</template>

<style scoped>
.faq {
  width: min(40rem, 100%);
  margin-top: calc(var(--space-6) * 2);
}

h2 {
  font-size: 1.125rem;
  margin: 0 0 var(--space-3);
}

details {
  border-bottom: 1px solid var(--color-border);
  padding: var(--space-3) 0;
}

summary {
  cursor: pointer;
  font-weight: 600;
}

p {
  color: var(--color-text-muted);
  margin: var(--space-2) 0 0;
}
</style>
