// @vitest-environment node
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { dailyApi } from '../../src/api/daily';
import { fallbacksApi } from '../../src/api/fallbacks';
import { mixApi } from '../../src/api/mix';
import { modelsApi } from '../../src/api/models';
import type { Scope } from '../../src/api/params';
import { reposApi } from '../../src/api/repos';
import { rolesApi } from '../../src/api/roles';
import { runsApi } from '../../src/api/runs';
import { savingsApi } from '../../src/api/savings';
import { sourcesApi } from '../../src/api/sources';
import { statusApi } from '../../src/api/status';
import { summaryApi } from '../../src/api/summary';
import { thinkingApi } from '../../src/api/thinking';
import { topRunsApi } from '../../src/api/topRuns';
import { API_PATHS, fixtureFor } from '../fixtures/mockup';

// The page's side of the API contract. The fixtures type-check against src/types.ts (tsc), and
// their JSON is committed under tests/fixtures/json for the server's contract tests (S3a) to
// read: this test fails when the two drift. UPDATE_FIXTURES=1 rewrites the JSON.

const every: Scope = {
  range: 'custom',
  from: '2026-10-01',
  to: '2026-10-26',
  workspace: '~/gitRepos',
  repo: 'inkcast',
  tool: 'Claude Code',
  model: 'claude-opus-5-5',
  effort: 'xhigh',
};

describe('each endpoint takes exactly the query its dataset line names', () => {
  const range = 'from=2026-10-01&to=2026-10-26';
  it.each([
    ['status', statusApi.url(undefined), '/api/status'],
    ['summary', summaryApi.url(every), `/api/summary?${range}&repo=inkcast&tool=Claude+Code&model=claude-opus-5-5&effort=xhigh`],
    ['days', dailyApi.url(every), `/api/daily?${range}&repo=inkcast&tool=Claude+Code&model=claude-opus-5-5&effort=xhigh`],
    ['mix', mixApi.url(every), `/api/mix?${range}&repo=inkcast&tool=Claude+Code&model=claude-opus-5-5&effort=xhigh`],
    ['topRuns', topRunsApi.url(every), `/api/runs/top?limit=5&${range}&repo=inkcast&tool=Claude+Code&model=claude-opus-5-5&effort=xhigh`],
    ['runs', runsApi.url(every), `/api/runs?${range}&repo=inkcast&tool=Claude+Code&model=claude-opus-5-5&effort=xhigh`],
    ['roles', rolesApi.url(every), `/api/roles?${range}&repo=inkcast&tool=Claude+Code&model=claude-opus-5-5&effort=xhigh`],
    ['models', modelsApi.url(every), `/api/models?${range}&repo=inkcast&tool=Claude+Code`],
    ['thinking', thinkingApi.url(every), `/api/thinking?${range}&repo=inkcast&tool=Claude+Code`],
    ['fallbacks', fallbacksApi.url(every), `/api/fallbacks?${range}&repo=inkcast&tool=Claude+Code`],
    ['repos', reposApi.url(every), `/api/repos?workspace=%7E%2FgitRepos&${range}&tool=Claude+Code`],
    ['savings', savingsApi.url(every), `/api/savings?${range}&repo=inkcast`],
    ['sources', sourcesApi.url(undefined), '/api/sources'],
  ])('%s', (_name, url, expected) => {
    expect(url).toBe(expected);
  });

  it('a preset range sends range alone, and "all" filters are not sent', () => {
    const preset: Scope = { range: '45', from: '2026-09-12', to: '2026-10-26', workspace: '', repo: '', tool: '', model: '', effort: '' };
    expect(summaryApi.url(preset)).toBe('/api/summary?range=45');
    expect(reposApi.url(preset)).toBe('/api/repos?range=45');
  });
});

describe('the committed JSON fixtures match the typed fixtures', () => {
  const dir = fileURLToPath(new URL('../fixtures/json/', import.meta.url));
  const fileOf = (path: string) => `${dir}${path.slice('/api/'.length).replace('/', '-')}.json`;
  const query = new URLSearchParams({ range: '30', limit: '5' });

  if (process.env.UPDATE_FIXTURES) {
    mkdirSync(dir, { recursive: true });
    for (const path of API_PATHS) writeFileSync(fileOf(path), `${JSON.stringify(fixtureFor(path, query), null, 2)}\n`);
  }

  it.each(API_PATHS)('%s', (path) => {
    expect(JSON.parse(readFileSync(fileOf(path), 'utf8'))).toEqual(fixtureFor(path, query));
  });
});
