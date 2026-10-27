import { cleanup, render, screen, waitFor } from '@testing-library/preact';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { App } from '../../src/App';
import { nestRuns } from '../../src/tabs/Agents';
import { heatCell } from '../../src/tabs/Models';
import type { Run } from '../../src/types';
import { fixtureFor } from '../fixtures/mockup';

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe('heat cells', () => {
  // The mockup sets the level and the text from the same value; a cell must never print "$10" in
  // the colour of the band below $10.
  it.each([
    [9.94, '$9.9', 0],
    [9.96, '$10', 1],
    [39.6, '$40', 2],
    [99.6, '$100', 3],
  ] as const)('%s prints %s at level %s', (cost, text, level) => {
    expect(heatCell(cost, 'm', 'high')).toMatchObject({ text, level });
  });
});

describe('nestRuns', () => {
  const run = (id: string, parentId: string | null): Run => ({
    id,
    parentId,
    name: id,
    role: 'main',
    tool: 'Codex',
    model: 'm',
    effort: null,
    inputTokens: 0,
    outputTokens: 0,
    costUsd: 0,
    state: 'finished',
  });

  it('shows every run once when the server repeats an id', () => {
    const runs = [run('x', null), run('x', 'x')];
    expect(nestRuns(runs)).toEqual(runs);
  });

  it('shows runs whose parents form a loop instead of dropping them', () => {
    const runs = [run('a', 'b'), run('b', 'a'), run('c', 'c')];
    expect(nestRuns(runs)).toHaveLength(3);
  });
});

it('a response that does not fit the contract shows an error, not endless loading', async () => {
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL(String(input), 'http://127.0.0.1:7311');
      const body = url.pathname === '/api/mix' ? {} : fixtureFor(url.pathname, url.searchParams);
      return new Response(JSON.stringify(body), { status: 200 });
    }),
  );
  window.history.replaceState(null, '', '/');
  render(<App />);
  await waitFor(() => expect(screen.getByRole('alert').textContent).toContain("Couldn't draw this tab"));
  expect(document.querySelectorAll('[aria-busy="true"]')).toHaveLength(0);
});
