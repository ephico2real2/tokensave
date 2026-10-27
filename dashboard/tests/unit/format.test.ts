import { describe, expect, it } from 'vitest';
import { K, M1, M2, ago, heatUsd, int, modelLabel, pct, pct1, rangeText, shortModel, ticks, usd } from '../../src/format';
import { MOCKUP_NOW } from '../fixtures/mockup';

// Expected strings are the ones the mockup draws from the same numbers.
describe('format', () => {
  it('formats tokens', () => {
    expect(M1(48.2e6)).toBe('48.2M');
    expect(M2(2.31e6)).toBe('2.31M');
    expect(M2(0.97e6)).toBe('0.97M');
    expect(K(412e3)).toBe('412k');
    expect(K(1.81e6)).toBe('1.81M');
    expect(K(4e3)).toBe('4k');
    expect(K(null)).toBe('—');
  });

  it('formats money, shares and counts', () => {
    expect(usd(312.4)).toBe('$312.40');
    expect(usd(0.4)).toBe('$0.40');
    expect(usd(null)).toBe('—');
    expect(pct(0.82)).toBe('82%');
    expect(pct1(0.125)).toBe('12.5%');
    expect(pct1(0.004)).toBe('0.4%');
    expect(int(18402)).toBe('18,402');
    expect(int(null)).toBe('—');
  });

  it('labels models with their effort', () => {
    expect(shortModel('claude-opus-5-5')).toBe('opus-5-5');
    expect(shortModel('gpt-6.1-sol')).toBe('gpt-6.1-sol');
    expect(modelLabel('claude-fable-5-1', 'xhigh')).toBe('fable-5-1 · xhigh');
    expect(modelLabel('grok-4.7-high', null)).toBe('grok-4.7-high');
  });

  it('prints grid costs in whole dollars from $10, one decimal below', () => {
    expect(heatUsd(128)).toBe('$128');
    expect(heatUsd(54.37)).toBe('$54');
    expect(heatUsd(9)).toBe('$9');
    expect(heatUsd(1.26)).toBe('$1.3');
  });

  it('says how long ago', () => {
    const minutesAgo = (m: number) => new Date(MOCKUP_NOW - m * 60_000).toISOString();
    expect(ago(minutesAgo(2), MOCKUP_NOW)).toBe('2 min ago');
    expect(ago(minutesAgo(130), MOCKUP_NOW)).toBe('2 h ago');
    expect(ago(minutesAgo(1440), MOCKUP_NOW)).toBe('yesterday');
    expect(ago(minutesAgo(4 * 1440), MOCKUP_NOW)).toBe('4 days ago');
    expect(ago(minutesAgo(7 * 1440), MOCKUP_NOW)).toBe('1 week ago');
    expect(ago(minutesAgo(21 * 1440), MOCKUP_NOW)).toBe('3 weeks ago');
    expect(ago(null, MOCKUP_NOW)).toBe('never');
  });

  it('says the range in words', () => {
    expect(rangeText('2026-09-27', '2026-10-26')).toBe('Sep 27 – Oct 26, 2026 · 30 days');
    expect(rangeText('2026-10-26', '2026-10-26')).toBe('Oct 26 – Oct 26, 2026 · 1 day');
    expect(rangeText('2025-12-30', '2026-01-02')).toBe('Dec 30, 2025 – Jan 2, 2026 · 4 days');
  });

  it('puts five ticks on the axis', () => {
    const dates = Array.from({ length: 30 }, (_, i) => new Date(Date.UTC(2026, 8, 27 + i)).toISOString().slice(0, 10));
    expect(ticks(dates)).toEqual(['Sep 27', 'Oct 4', 'Oct 12', 'Oct 19', 'Oct 26']);
    expect(ticks([])).toEqual([]);
  });
});
