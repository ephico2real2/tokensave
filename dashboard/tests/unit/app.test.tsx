import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/preact';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { App } from '../../src/App';
import { MOCKUP_NOW } from '../fixtures/mockup';
import { mockApi } from './api-mock';

beforeEach(() => {
  vi.useFakeTimers({ now: MOCKUP_NOW, toFake: ['Date'] });
  window.localStorage.clear();
  window.history.replaceState(null, '', '/');
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
  delete document.documentElement.dataset.theme;
});

function open(search = '') {
  window.history.replaceState(null, '', `/${search}`);
  render(<App />);
}

const section = (type: string, dataset?: string) =>
  [...document.querySelectorAll<HTMLElement>(`[data-component="${type}"]`)].filter(
    (el) => !dataset || el.dataset.dataset === dataset,
  );

/** Waits until no section is loading. */
const loaded = () =>
  waitFor(() => {
    expect(document.querySelectorAll('[aria-busy="true"]')).toHaveLength(0);
    expect(document.body.textContent).not.toContain('connecting…');
  });

// Each tab against the mockup's sample data: the text each section shows is the mockup's.
describe('tabs render the fixtures as the mockup does', () => {
  it('Overview', async () => {
    mockApi('fixture');
    open();
    await loaded();
    expect(screen.getByText('Dashboard · 127.0.0.1:7311')).toBeTruthy();
    expect(screen.getByText('Sep 27 – Oct 26, 2026 · 30 days')).toBeTruthy();
    const tiles = section('kpi-tiles')[0]!.textContent;
    for (const t of ['48.2M', '$312.40', '6.9M', '12.5%', '71%', 'from the embedded price table']) expect(tiles).toContain(t);
    expect(section('time-series')[0]!.querySelectorAll('.ts-plot__day')).toHaveLength(30);
    const [byTool, byModel] = section('bar-list', 'mix');
    expect(byTool!.textContent).toContain('Claude Code39.5M · 82%');
    expect(byModel!.textContent).toContain('claude-opus-5-529.4M · 61%');
    const top = section('row-list', 'topRuns')[0]!;
    expect(top.querySelectorAll('.ts-line')).toHaveLength(5);
    expect(top.textContent).toContain('Review PR #178reviewer · inkcastopus-5-5 · xhigh2.31M$18.70');
  });

  it('Agents', async () => {
    mockApi('fixture');
    open('?tab=agents');
    await loaded();
    const runs = section('row-list', 'runs')[0]!;
    const rows = runs.querySelectorAll('.ts-line');
    expect(rows).toHaveLength(7);
    expect(rows[0]!.textContent).toBe('inkcast · main sessionmain · Claude Codeopus-5-5 · high412k / 38k$7.90running');
    expect(rows[1]!.classList.contains('ts-line--child')).toBe(true);
    expect(rows[6]!.textContent).toContain('grok-4.7-high120k / 9k$0.80finished');
    const roles = section('bar-list', 'roles')[0]!;
    expect(roles.tagName).toBe('ASIDE');
    expect(roles.textContent).toContain('reviewer$128.40 · 41%');
    expect(roles.textContent).toContain('Subagent share of spend78%');
    expect(roles.textContent).toContain("Runs that didn't finish4");
  });

  it('Models and effort', async () => {
    mockApi('fixture');
    open('?tab=models');
    await loaded();
    const grid = within(section('heat-grid')[0]!);
    expect(grid.getAllByRole('row')).toHaveLength(5);
    expect(grid.getByRole('cell', { name: 'claude-opus-5-5, xhigh: $128' }).className).toContain('ts-heat__cell--3');
    expect(grid.getByRole('cell', { name: 'claude-sonnet-5-5, low: $1' }).className).toContain('ts-heat__cell--0');
    expect(grid.getByRole('cell', { name: 'gpt-6.1-sol, max: no turns' }).textContent).toBe('—');
    expect(section('bar-list', 'thinking')[0]!.textContent).toContain('opus-5-5 · xhigh64%');
    const fb = section('row-list', 'fallbacks')[0]!.textContent;
    expect(fb).toContain('fable-5-1 → opus-5-531 turns · 2 sessions');
    expect(fb).toContain('opus-5-5 → sonnet-5-54 turns · 1 session');
    expect(fb).toContain('Fallback rate0.4%');
  });

  it('Repos', async () => {
    mockApi('fixture');
    open('?tab=repos');
    await loaded();
    const strip = section('stat-strip')[0]!.textContent;
    expect(strip).toContain('~/gitRepos');
    expect(strip).toContain('marked by .tokensave-workspace.toml · scan depth 1');
    expect(strip).toContain('Repos6Indexed4Not indexed2');
    const rows = section('row-list', 'repos')[0]!.querySelectorAll('.ts-line');
    expect(rows).toHaveLength(6);
    expect(rows[0]!.textContent).toBe('inkcast~/gitRepos/inkcastindexedSpent 41.3MSaved 6.2MClaude Code2 min ago');
    expect(rows[5]!.textContent).toBe('data-lab~/gitRepos/data-labnot indexedSpent —Saved —never');
    expect(section('callout')[0]!.textContent).toBe(
      "2 repos aren't indexed, so tokensave saves nothing theretokensave workspace init-all --dry-run",
    );
  });

  it('Savings', async () => {
    mockApi('fixture');
    open('?tab=savings');
    await loaded();
    const tiles = section('kpi-tiles', 'savings')[0]!.textContent;
    for (const t of ['6.9M', 'before 7.8M → after 0.9M', '2,418', '3.4M']) expect(tiles).toContain(t);
    expect(section('bar-list', 'savings')[0]!.textContent).toContain('tokensave_context2.9M · 42%');
    expect(section('row-list', 'savings')[0]!.textContent).toContain(
      'Whole-file reads2.1M · 1,204 turnsUse instead: outline / read / node',
    );
  });

  it('Sources', async () => {
    mockApi('fixture');
    open('?tab=sources');
    await loaded();
    const cards = section('card-grid')[0]!.querySelectorAll('.ts-card--source');
    expect(cards).toHaveLength(5);
    expect(cards[0]!.textContent).toContain('Claude Codeverified~/.claude/projects/Turns18,402Last parsed2 min ago');
    expect(cards[3]!.textContent).toContain('Pinot found~/.pi/agent/sessions/Turns—Last parsed—');
  });
});

const TABS = ['overview', 'agents', 'models', 'repos', 'savings', 'sources'];

describe('every section has its empty and error state', () => {
  it.each(TABS)('%s, empty', async (tab) => {
    mockApi('empty');
    open(`?tab=${tab}`);
    await loaded();
    // Every section on the tab says why it has nothing, inside itself.
    for (const el of document.querySelectorAll<HTMLElement>('main [data-component]')) {
      expect(el.querySelector('.ts-state'), `${el.dataset.component}/${el.dataset.dataset}`).not.toBeNull();
    }
    expect(document.querySelector('[data-component="callout"]')).toBeNull();
  });

  it.each(TABS)('%s, error, then Retry', async (tab) => {
    let failing = true;
    const calls = mockApi(() => (failing ? 'error' : 'fixture'));
    open(`?tab=${tab}`);
    await loaded();
    const sections = [...document.querySelectorAll<HTMLElement>('main [data-component]')];
    expect(sections.length).toBeGreaterThan(0);
    for (const el of sections) {
      expect(el.querySelector('[role="alert"]')?.textContent, `${el.dataset.component}`).toContain('500 Internal Server Error (database is locked)');
    }
    expect(screen.getByText(/server not reachable/)).toBeTruthy();

    failing = false;
    const before = calls.length;
    fireEvent.click(within(sections[0]!).getByRole('button', { name: 'Retry' }));
    await loaded();
    expect(calls.length).toBeGreaterThan(before);
    expect(sections[0]!.querySelector('[role="alert"]')).toBeNull();
  });

  it('shows loading while a request is out', async () => {
    mockApi('pending');
    open();
    await waitFor(() => expect(document.querySelectorAll('main [aria-busy="true"]').length).toBe(5));
    expect(screen.getByText('Dashboard · connecting…')).toBeTruthy();
  });
});

describe('state', () => {
  it('a tab, the range and a filter go to the URL and to every request', async () => {
    const calls = mockApi('fixture');
    open();
    await loaded();
    expect(calls).toContain('/api/summary?range=30');
    expect(calls).toContain('/api/runs/top?limit=5&range=30');
    expect(calls).toContain('/api/repos?range=30');

    fireEvent.click(screen.getByRole('button', { name: '7 days' }));
    await loaded();
    expect(window.location.search).toBe('?range=7');
    expect(calls).toContain('/api/daily?range=7');
    expect(screen.getByRole('button', { name: '7 days' }).getAttribute('aria-pressed')).toBe('true');
    expect(section('time-series')[0]!.querySelectorAll('.ts-plot__day')).toHaveLength(7);

    fireEvent.change(screen.getByRole('combobox', { name: /Agent tool/ }), { target: { value: 'Codex' } });
    await loaded();
    expect(window.location.search).toBe('?range=7&tool=Codex');
    expect(calls).toContain('/api/mix?range=7&tool=Codex');

    fireEvent.click(screen.getByRole('button', { name: 'Models and effort' }));
    await loaded();
    expect(window.location.search).toBe('?tab=models&range=7&tool=Codex');
    expect(screen.getByRole('button', { name: 'Models and effort' }).getAttribute('aria-current')).toBe('page');
    // The models endpoint takes repo and tool, not model or effort.
    expect(calls).toContain('/api/models?range=7&tool=Codex');
  });

  it('asks for each dataset once per scope, though the filters and a tab share it', async () => {
    const calls = mockApi('fixture');
    open('?tab=repos');
    await loaded();
    expect(calls.filter((c) => c === '/api/repos?range=30')).toHaveLength(1);
    fireEvent.click(screen.getByRole('button', { name: 'Models and effort' }));
    await loaded();
    expect(calls.filter((c) => c === '/api/models?range=30')).toHaveLength(1);
    expect(calls.filter((c) => c === '/api/repos?range=30')).toHaveLength(1);
  });

  it('Custom opens on the 30 days to today and sends from and to', async () => {
    const calls = mockApi('fixture');
    open();
    await loaded();
    fireEvent.click(screen.getByRole('button', { name: 'Custom' }));
    await loaded();
    expect(window.location.search).toBe('?range=custom&from=2026-09-27&to=2026-10-26');
    const from = screen.getByLabelText('From') as HTMLInputElement;
    const to = screen.getByLabelText('To') as HTMLInputElement;
    expect(to.max).toBe('2026-10-26');
    fireEvent.change(from, { target: { value: '2026-10-01' } });
    await loaded();
    expect(calls).toContain('/api/summary?from=2026-10-01&to=2026-10-26');
    expect(screen.getByText('Oct 1 – Oct 26, 2026 · 26 days')).toBeTruthy();
  });

  it('filter choices come from the repos and models in the range, and a shared value stays', async () => {
    mockApi('fixture');
    open('?repo=old-repo');
    await loaded();
    const repo = screen.getByRole('combobox', { name: /Repo/ }) as HTMLSelectElement;
    expect([...repo.options].map((o) => o.textContent)).toEqual([
      'All repos',
      'inkcast',
      'service-a',
      'secrets-demo',
      'docs-site',
      'infra-lab',
      'data-lab',
      'old-repo',
    ]);
    expect(repo.value).toBe('old-repo');
    const model = screen.getByRole('combobox', { name: /Model/ }) as HTMLSelectElement;
    expect([...model.options].map((o) => o.value)).toEqual(['', 'claude-opus-5-5', 'claude-fable-5-1', 'claude-sonnet-5-5', 'gpt-6.1-sol']);
    const ws = screen.getByRole('combobox', { name: /Workspace/ }) as HTMLSelectElement;
    expect([...ws.options].map((o) => o.textContent)).toEqual(['~/gitRepos', 'All workspaces']);
    expect(ws.value).toBe('');
  });

  it('the theme: Dark by default, remembered, never in the URL', async () => {
    mockApi('fixture');
    open();
    await loaded();
    expect(document.documentElement.dataset.theme).toBe('dark');
    expect(screen.getByRole('button', { name: 'Dark' }).getAttribute('aria-pressed')).toBe('true');
    fireEvent.click(screen.getByRole('button', { name: 'Light' }));
    expect(document.documentElement.dataset.theme).toBe('light');
    expect(window.localStorage.getItem('theme')).toBe('light');
    expect(window.location.search).toBe('');
    cleanup();
    open();
    await loaded();
    expect(document.documentElement.dataset.theme).toBe('light');
  });

  it('the Filters button opens and closes the filter sheet', async () => {
    mockApi('fixture');
    open('?repo=inkcast&tool=Codex');
    await loaded();
    const button = screen.getByRole('button', { name: 'Filters · inkcast, Codex' });
    expect(button.getAttribute('aria-expanded')).toBe('false');
    fireEvent.click(button);
    expect(button.getAttribute('aria-expanded')).toBe('true');
    expect(document.getElementById(button.getAttribute('aria-controls')!)?.classList.contains('is-open')).toBe(true);
  });
});
