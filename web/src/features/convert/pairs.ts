/** Oldest and newest Revit versions the engine converts between. */
export const OLDEST = 2023;
export const NEWEST = 2027;

export interface Pair {
  from: number;
  to: number;
}

/** Every downgrade with a page of its own, newest source first. */
export const PAIRS: Pair[] = Array.from({ length: NEWEST - OLDEST }, (_, i) => NEWEST - i).flatMap((from) =>
  Array.from({ length: from - OLDEST }, (_, j) => ({ from, to: from - 1 - j })),
);

export function isPair(from: number, to: number): boolean {
  return PAIRS.some((p) => p.from === from && p.to === to);
}

/** Path of a pair's page, e.g. /revit-2026-to-2024. */
export function pairPath({ from, to }: Pair): string {
  return `/revit-${String(from)}-to-${String(to)}`;
}
