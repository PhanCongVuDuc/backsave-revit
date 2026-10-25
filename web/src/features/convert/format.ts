const UNITS = ['B', 'KB', 'MB', 'GB'] as const;

/** Binary units (1 KB = 1024 B); one decimal under 10, none above. */
export function formatBytes(bytes: number): string {
  let value = Math.max(0, bytes);
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const rounded = unit === 0 || value >= 10 ? Math.round(value) : Math.round(value * 10) / 10;
  return `${String(rounded)} ${UNITS[unit] ?? 'B'}`;
}

/** Seconds as m:ss. */
export function formatClock(seconds: number): string {
  const whole = Math.max(0, Math.round(seconds));
  const minutes = Math.floor(whole / 60);
  return `${String(minutes)}:${String(whole % 60).padStart(2, '0')}`;
}

/** Hour and minute of an RFC 3339 time in the viewer's time zone and the given locale. */
export function formatTimeOfDay(iso: string, locale: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) {
    return '';
  }
  return new Intl.DateTimeFormat(locale, { hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }).format(date);
}
