<script setup lang="ts">
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';

import BaseSelect from '@/shared/ui/BaseSelect.vue';

const props = defineProps<{ targets: number[] }>();
const model = defineModel<number>({ required: true });
const { t } = useI18n();
const options = computed(() =>
  props.targets.map((year) => ({ value: year, label: t('convert.target.option', { year }) })),
);
</script>

<template>
  <BaseSelect v-model="model" :options="options" :label="t('convert.target.label')" />
</template>
