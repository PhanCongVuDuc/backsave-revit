// The public usage totals shown on the home page. Components reach @/api only through a store module.
export { getStats as loadUsageStats } from '@/api';
export type { UsageStats } from '@/api';
