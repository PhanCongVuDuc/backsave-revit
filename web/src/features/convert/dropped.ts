/** The shape of one dropped element; matches what the store holds (`DroppedElement` in the API types). */
export interface DroppedItem {
  elementId: number;
  class: string;
  reason: 'no_rule' | 'rule_broken' | 'cascade';
  cascadeFrom: number | null;
}

export interface DroppedGroup {
  class: string;
  reason: DroppedItem['reason'];
  count: number;
}

/** One row per class and reason, the biggest first (ties by class name, then reason, so the order is stable). */
export function groupDropped(list: readonly DroppedItem[]): DroppedGroup[] {
  const groups = new Map<string, DroppedGroup>();
  for (const item of list) {
    const key = `${item.reason}\u0000${item.class}`;
    const group = groups.get(key);
    if (group) {
      group.count += 1;
    } else {
      groups.set(key, { class: item.class, reason: item.reason, count: 1 });
    }
  }
  return [...groups.values()].sort(
    (a, b) => b.count - a.count || a.class.localeCompare(b.class) || a.reason.localeCompare(b.reason),
  );
}

export const CSV_HEADER = 'element_id,class,reason,cascade_from';

function field(value: string): string {
  return /[",\r\n]/.test(value) ? `"${value.replaceAll('"', '""')}"` : value;
}

/** The list as CSV text with a UTF-8 byte order mark, so spreadsheet programs open it correctly. */
export function toCsv(list: readonly DroppedItem[]): string {
  const rows = list.map((item) =>
    [
      String(item.elementId),
      field(item.class),
      item.reason,
      item.cascadeFrom === null ? '' : String(item.cascadeFrom),
    ].join(','),
  );
  return `\uFEFF${[CSV_HEADER, ...rows].join('\r\n')}\r\n`;
}
