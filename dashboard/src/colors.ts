// Category colours: which token marks which thing. Fixed for tools and roles on every tab;
// models take the set in order of spend (the mockup's TOOL, ROLE and MODEL maps).
// Values are CSS variable references; the colours themselves live only in tokens.css.

import type { Mix, Role, RunState, SourceStatus, Tool } from './types';

export type Cat = 'emerald' | 'sky' | 'amber' | 'violet' | 'coral' | 'pink' | 'teal' | 'slate';

export const cat = (name: Cat): string => `var(--cat-${name})`;

export const TOOL_COLOR: Record<Tool, string> = {
  'Claude Code': cat('coral'),
  Codex: cat('sky'),
  Cursor: cat('violet'),
  Pi: cat('amber'),
  'Factory Droid': cat('pink'),
};

export const ROLE_COLOR: Record<Role, string> = {
  builder: cat('emerald'),
  reviewer: cat('violet'),
  main: cat('slate'),
  Explore: cat('amber'),
  other: cat('teal'),
};

const MODEL_ORDER: readonly Cat[] = ['emerald', 'sky', 'amber', 'violet', 'coral', 'pink', 'teal'];

/** Models by spend (Mix.byModel order) to their colour; models past the set are slate. */
export function modelColors(mix: Mix | undefined): (model: string) => string {
  const byModel = new Map<string, string>();
  mix?.byModel.forEach((m, i) => byModel.set(m.model, cat(MODEL_ORDER[i] ?? 'slate')));
  return (model) => byModel.get(model) ?? cat('slate');
}

/** An icon tile: the category colour at --tint over the card surface. */
export const tileOf = (color: string): string => `color-mix(in srgb, ${color} var(--tint), var(--surface))`;

export type Tone = 'none' | 'ok' | 'warn' | 'info' | 'stop';

export const RUN_TONE: Record<RunState, Tone> = { finished: 'ok', running: 'info', stopped: 'stop', partial: 'warn' };

export const SOURCE_TONE: Record<SourceStatus, Tone> = {
  verified: 'ok',
  partial: 'warn',
  'not found': 'none',
  unknown: 'warn',
};
