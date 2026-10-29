<script setup lang="ts">
import { computed, onMounted, ref } from 'vue';
import { useI18n } from 'vue-i18n';

import { loadUsageStats, type UsageStats as UsageStatsData } from '../stores/usageStats';

const KEYS = ['people', 'succeeded', 'failed', 'contributed'] as const;

const { t, locale } = useI18n();
const stats = ref<UsageStatsData | null>(null);

// Read once per page load; the numbers do not refresh while the page is open.
onMounted(() => {
  void loadUsageStats().then((loaded) => {
    stats.value = loaded;
  });
});

const items = computed(() => {
  const current = stats.value;
  if (!current) {
    return [];
  }
  const number = new Intl.NumberFormat(locale.value);
  return KEYS.map((key) => ({ key, value: number.format(current[key]) }));
});

const since = computed(() =>
  stats.value
    ? new Intl.DateTimeFormat(locale.value, { timeZone: 'UTC', dateStyle: 'long' }).format(
        new Date(`${stats.value.since}T00:00:00Z`),
      )
    : '',
);
</script>

<template>
  <section v-if="stats" class="usage" :aria-label="t('convert.stats.label')">
    <dl>
      <div v-for="item in items" :key="item.key" class="item">
        <dt>{{ t(`convert.stats.${item.key}`) }}</dt>
        <dd>{{ item.value }}</dd>
      </div>
    </dl>
    <p class="since">{{ t('convert.stats.since', { date: since }) }}</p>
    <p class="help">{{ t('convert.stats.help') }}</p>
  </section>
</template>

<style scoped>
.usage {
  width: min(40rem, 100%);
  margin-top: var(--space-6);
  text-align: center;
}

dl {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(8rem, 1fr));
  gap: var(--space-4);
  margin: 0;
}

/* The number shows above its label; numbers stay on one line even when a label wraps. */
.item {
  display: flex;
  flex-direction: column-reverse;
  justify-content: flex-end;
  gap: var(--space-1);
}

dt {
  color: var(--color-text-muted);
  font-size: 0.875rem;
}

dd {
  margin: 0;
  font-size: 1.5rem;
  font-weight: 700;
}

.since {
  margin: var(--space-3) 0 0;
  color: var(--color-text-muted);
  font-size: 0.8125rem;
}

.help {
  margin: var(--space-2) auto 0;
  max-width: 32rem;
  color: var(--color-text-muted);
  font-size: 0.8125rem;
}
</style>
