import { describe, expect, it } from 'vitest';

import { CSV_HEADER, type DroppedItem, groupDropped, toCsv } from './dropped';

const LIST: DroppedItem[] = [
  { elementId: 10, class: 'Wall', reason: 'no_rule', cascadeFrom: null },
  { elementId: 11, class: 'Duct', reason: 'rule_broken', cascadeFrom: null },
  { elementId: 12, class: 'Wall', reason: 'no_rule', cascadeFrom: null },
  { elementId: 13, class: 'Tag', reason: 'cascade', cascadeFrom: 10 },
  { elementId: 14, class: 'Wall', reason: 'no_rule', cascadeFrom: null },
  { elementId: 15, class: 'Beam', reason: 'cascade', cascadeFrom: 11 },
];

describe('groupDropped', () => {
  it('counts per class and reason, biggest first', () => {
    expect(groupDropped(LIST)).toEqual([
      { class: 'Wall', reason: 'no_rule', count: 3 },
      { class: 'Beam', reason: 'cascade', count: 1 },
      { class: 'Duct', reason: 'rule_broken', count: 1 },
      { class: 'Tag', reason: 'cascade', count: 1 },
    ]);
  });

  it('keeps one class apart when the reasons differ', () => {
    const groups = groupDropped([
      { elementId: 1, class: 'Wall', reason: 'no_rule', cascadeFrom: null },
      { elementId: 2, class: 'Wall', reason: 'cascade', cascadeFrom: 1 },
    ]);
    expect(groups).toHaveLength(2);
  });

  it('gives nothing for an empty list', () => {
    expect(groupDropped([])).toEqual([]);
  });
});

describe('toCsv', () => {
  it('csv_has_bom_and_header', () => {
    const csv = toCsv(LIST);
    expect(csv.codePointAt(0)).toBe(0xfeff);
    expect(csv.slice(1).split('\r\n')[0]).toBe(CSV_HEADER);
    expect(CSV_HEADER).toBe('element_id,class,reason,cascade_from');
  });

  it('writes one line per element, with an empty cascade_from when there is none', () => {
    const lines = toCsv(LIST).slice(1).trimEnd().split('\r\n');
    expect(lines).toHaveLength(LIST.length + 1);
    expect(lines[1]).toBe('10,Wall,no_rule,');
    expect(lines[4]).toBe('13,Tag,cascade,10');
  });

  it('quotes a class name that holds a comma or a quote', () => {
    const csv = toCsv([{ elementId: 1, class: 'A,"B"', reason: 'no_rule', cascadeFrom: null }]);
    expect(csv).toContain('1,"A,""B""",no_rule,');
  });
});
