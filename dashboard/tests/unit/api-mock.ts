import { vi } from 'vitest';
import { emptyFor, fixtureFor } from '../fixtures/mockup';

export type Mode = 'fixture' | 'empty' | 'error' | 'pending';

/** Answers every /api/ request from the fixtures (or empty, failing, never); returns the requests made. */
export function mockApi(mode: Mode | (() => Mode)): string[] {
  const calls: string[] = [];
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL(String(input), 'http://127.0.0.1:7311');
      calls.push(`${url.pathname}${url.search}`);
      const m = typeof mode === 'function' ? mode() : mode;
      if (m === 'pending') return new Promise<Response>(() => {});
      if (m === 'error') return new Response('database is locked', { status: 500, statusText: 'Internal Server Error' });
      const body = m === 'empty' ? emptyFor(url.pathname) : fixtureFor(url.pathname, url.searchParams);
      return new Response(JSON.stringify(body), { status: 200, headers: { 'Content-Type': 'application/json' } });
    }),
  );
  return calls;
}
