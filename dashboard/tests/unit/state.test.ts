import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  clampCustom,
  isIsoDate,
  parseUrlState,
  rangeQuery,
  readStoredTheme,
  serializeUrlState,
  storeTheme,
  type UrlState,
} from '../../src/state';

const TODAY = '2026-10-26';

describe('URL state', () => {
  it('opens on the defaults', () => {
    expect(parseUrlState('', TODAY)).toEqual({
      tab: 'overview',
      range: '30',
      from: '2026-09-27',
      to: TODAY,
      workspace: '',
      repo: '',
      tool: '',
      model: '',
      effort: '',
    });
  });

  it('reads every key', () => {
    const s = parseUrlState(
      '?tab=models&range=custom&from=2026-10-01&to=2026-10-20&workspace=~/gitRepos&repo=inkcast&tool=Codex&model=gpt-6.1-sol&effort=high',
      TODAY,
    );
    expect(s).toEqual({
      tab: 'models',
      range: 'custom',
      from: '2026-10-01',
      to: '2026-10-20',
      workspace: '~/gitRepos',
      repo: 'inkcast',
      tool: 'Codex',
      model: 'gpt-6.1-sol',
      effort: 'high',
    });
  });

  it('falls back to the default for an invalid value', () => {
    const s = parseUrlState('?tab=billing&range=365&tool=Copilot&effort=extreme', TODAY);
    expect([s.tab, s.range, s.tool, s.effort]).toEqual(['overview', '30', '', '']);
  });

  it('accepts a custom range only when From ≤ To ≤ today and both are real dates', () => {
    const defaults = { from: '2026-09-27', to: TODAY };
    const pick = (q: string) => {
      const s = parseUrlState(`?range=custom&${q}`, TODAY);
      return { from: s.from, to: s.to };
    };
    expect(pick('from=2026-10-01&to=2026-10-27')).toEqual(defaults);
    expect(pick('from=2026-10-20&to=2026-10-01')).toEqual(defaults);
    expect(pick('from=2026-02-30&to=2026-03-01')).toEqual(defaults);
    expect(pick('from=yesterday&to=today')).toEqual(defaults);
    expect(pick('from=2026-10-26&to=2026-10-26')).toEqual({ from: TODAY, to: TODAY });
  });

  it('ignores From and To outside a custom range', () => {
    const s = parseUrlState('?range=7&from=2026-01-01&to=2026-01-31', TODAY);
    expect(serializeUrlState(s)).toBe('?range=7');
  });

  it('writes only what differs from the defaults, and reads back the same state', () => {
    expect(serializeUrlState(parseUrlState('', TODAY))).toBe('');
    const q = '?tab=repos&range=custom&from=2026-10-01&to=2026-10-20&repo=inkcast&tool=Claude+Code';
    const s = parseUrlState(q, TODAY);
    expect(serializeUrlState(s)).toBe(q);
    expect(parseUrlState(serializeUrlState(s), TODAY)).toEqual(s);
  });

  it('sends a preset as range and a custom range as from and to', () => {
    const base = parseUrlState('', TODAY);
    expect(rangeQuery(base)).toEqual({ range: '30' });
    const custom: UrlState = { ...base, range: 'custom', from: '2026-10-01', to: '2026-10-20' };
    expect(rangeQuery(custom)).toEqual({ from: '2026-10-01', to: '2026-10-20' });
  });

  it('keeps a custom range valid', () => {
    expect(clampCustom('2026-10-01', '2026-11-30', TODAY)).toEqual({ from: '2026-10-01', to: TODAY });
    expect(clampCustom('2026-10-25', '2026-10-20', TODAY)).toEqual({ from: '2026-10-20', to: '2026-10-20' });
  });

  it('checks calendar dates', () => {
    expect(isIsoDate('2026-10-26')).toBe(true);
    expect(isIsoDate('2026-13-01')).toBe(false);
    expect(isIsoDate('26-10-2026')).toBe(false);
  });
});

describe('theme storage', () => {
  afterEach(() => {
    vi.restoreAllMocks();
    window.localStorage.clear();
  });

  it('remembers a valid choice and ignores anything else', () => {
    storeTheme('light');
    expect(readStoredTheme()).toBe('light');
    window.localStorage.setItem('theme', 'sepia');
    expect(readStoredTheme()).toBeUndefined();
  });

  it('works when the browser refuses storage', () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new DOMException('denied', 'SecurityError');
    });
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new DOMException('denied', 'SecurityError');
    });
    expect(() => storeTheme('auto')).not.toThrow();
    expect(readStoredTheme()).toBeUndefined();
  });
});
