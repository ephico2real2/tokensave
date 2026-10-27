// The mockup's sample data, typed against the API contract (src/types.ts): the values and
// formulas are copied from renderVals() in inkcast
// docs/tooling/tokensave-dashboard/mockups/source/Main.dc.html, so the page drawn from these
// fixtures is comparable with the mockup's screenshots. Test-only: nothing here ships.

import type {
  Day,
  Fallbacks,
  Mix,
  ModelEffort,
  RoleCost,
  Repos,
  Run,
  Savings,
  Source,
  Status,
  Summary,
  Thinking,
  TopRun,
} from '../../src/types';

/** The mockup's clock: "today" is 2026-10-26 and times are relative to noon UTC. */
export const MOCKUP_TODAY = '2026-10-26';
export const MOCKUP_NOW = Date.parse(`${MOCKUP_TODAY}T12:00:00Z`);

const DAY_MS = 86_400_000;
const at = (minutesAgo: number): string => new Date(MOCKUP_NOW - minutesAgo * 60_000).toISOString();

export const status: Status = { bind: '127.0.0.1', port: 7311, workspaces: ['~/gitRepos'] };

/** The range a request asks for: `range=N` ending today, or `from`/`to`. */
export function requestedDays(query: URLSearchParams, today = MOCKUP_TODAY): { end: string; n: number } {
  const from = query.get('from');
  const to = query.get('to');
  if (from && to) {
    return { end: to, n: Math.max(1, Math.round((Date.parse(`${to}T00:00:00Z`) - Date.parse(`${from}T00:00:00Z`)) / DAY_MS) + 1) };
  }
  return { end: today, n: Number(query.get('range') ?? '30') };
}

/** Summary scales with the range, as in the mockup (48.2M tokens per 30 days). */
export function summary(n: number): Summary {
  const scale = n / 30;
  return {
    spentTokens: 48.2e6 * scale,
    costUsd: 312.4 * scale,
    unpricedTurns: 0,
    savedTokens: 6.9e6 * scale,
    savingRate: 0.125,
    cacheHitRate: 0.71,
  };
}

/** One day per day of the range, from the mockup's generator (a weekly dip on weekends). */
export function daily(end: string, n: number): Day[] {
  const endMs = Date.parse(`${end}T00:00:00Z`);
  const startMs = endMs - (n - 1) * DAY_MS;
  const out: Day[] = [];
  for (let i = 0; i < n; i++) {
    const t = endMs / DAY_MS - (n - 1 - i);
    const dow = (((t % 7) + 7 + 4) % 7);
    const v = 60 + 30 * Math.sin(t / 5) + 15 * Math.sin(t * 1.7) - (dow === 0 || dow === 6 ? 32 : 0);
    const spent = Math.max(12, v);
    const saved = Math.max(2, v * (0.11 + 0.03 * Math.sin(t / 3)));
    out.push({
      date: new Date(startMs + i * DAY_MS).toISOString().slice(0, 10),
      spentTokens: spent * 1e5,
      savedTokens: saved * 1e5,
    });
  }
  return out;
}

export const mix: Mix = {
  byTool: [
    { tool: 'Claude Code', tokens: 39.5e6, share: 0.82 },
    { tool: 'Codex', tokens: 7.2e6, share: 0.15 },
    { tool: 'Cursor', tokens: 1.5e6, share: 0.03 },
  ],
  byModel: [
    { model: 'claude-opus-5-5', tokens: 29.4e6, share: 0.61 },
    { model: 'claude-fable-5-1', tokens: 13.0e6, share: 0.27 },
    { model: 'claude-sonnet-5-5', tokens: 4.3e6, share: 0.09 },
    { model: 'gpt-6.1-sol', tokens: 1.5e6, share: 0.03 },
  ],
};

export const topRuns: TopRun[] = [
  { id: 'r1', name: 'Review PR #178', role: 'reviewer', repo: 'inkcast', model: 'claude-opus-5-5', effort: 'xhigh', tokens: 2.31e6, costUsd: 18.7 },
  { id: 'r2', name: 'Build #226', role: 'builder', repo: 'inkcast', model: 'claude-fable-5-1', effort: 'xhigh', tokens: 2.04e6, costUsd: 16.1 },
  { id: 'r3', name: 'Review PR #217', role: 'reviewer', repo: 'inkcast', model: 'claude-opus-5-5', effort: 'xhigh', tokens: 1.88e6, costUsd: 15.2 },
  { id: 'r4', name: 'Build #213', role: 'builder', repo: 'inkcast', model: 'claude-fable-5-1', effort: 'xhigh', tokens: 1.21e6, costUsd: 9.6 },
  { id: 'r5', name: 'Main session', role: 'main', repo: 'inkcast', model: 'claude-opus-5-5', effort: 'high', tokens: 0.97e6, costUsd: 7.9 },
];

export const runs: Run[] = [
  { id: 's1', parentId: null, name: 'inkcast · main session', role: 'main', tool: 'Claude Code', model: 'claude-opus-5-5', effort: 'high', inputTokens: 412e3, outputTokens: 38e3, costUsd: 7.9, state: 'running' },
  { id: 'a1', parentId: 's1', name: 'Build #226', role: 'builder', tool: 'Claude Code', model: 'claude-fable-5-1', effort: 'xhigh', inputTokens: 1.81e6, outputTokens: 230e3, costUsd: 16.1, state: 'finished' },
  { id: 'a2', parentId: 's1', name: 'Review PR #178', role: 'reviewer', tool: 'Claude Code', model: 'claude-opus-5-5', effort: 'xhigh', inputTokens: 2.02e6, outputTokens: 290e3, costUsd: 18.7, state: 'finished' },
  { id: 'a3', parentId: 's1', name: 'Review PR #221 (first try)', role: 'reviewer', tool: 'Claude Code', model: 'claude-opus-5-5', effort: 'xhigh', inputTokens: 640e3, outputTokens: 80e3, costUsd: 5.3, state: 'stopped' },
  { id: 'a4', parentId: 's1', name: 'Find sync callers', role: 'Explore', tool: 'Claude Code', model: 'claude-sonnet-5-5', effort: 'medium', inputTokens: 96e3, outputTokens: 4e3, costUsd: 0.4, state: 'finished' },
  { id: 's2', parentId: null, name: 'service-a · session', role: 'main', tool: 'Codex', model: 'gpt-6.1-sol', effort: 'high', inputTokens: 380e3, outputTokens: 22e3, costUsd: 2.1, state: 'finished' },
  { id: 's3', parentId: null, name: 'infra-lab · session', role: 'main', tool: 'Cursor', model: 'grok-4.7-high', effort: null, inputTokens: 120e3, outputTokens: 9e3, costUsd: 0.8, state: 'finished' },
];

export const roles: RoleCost = {
  roles: [
    { role: 'reviewer', costUsd: 128.4, share: 0.41 },
    { role: 'builder', costUsd: 109.6, share: 0.35 },
    { role: 'main', costUsd: 62.5, share: 0.2 },
    { role: 'Explore', costUsd: 9.3, share: 0.03 },
    { role: 'other', costUsd: 2.6, share: 0.01 },
  ],
  subagentShare: 0.78,
  unfinishedRuns: 4,
};

export const models: ModelEffort = {
  efforts: ['low', 'medium', 'high', 'xhigh', 'max'],
  rows: [
    { model: 'claude-opus-5-5', costUsd: [null, 2, 54, 128, null] },
    { model: 'claude-fable-5-1', costUsd: [null, null, 18, 92, null] },
    { model: 'claude-sonnet-5-5', costUsd: [1, 9, 4, null, null] },
    { model: 'gpt-6.1-sol', costUsd: [null, 3, 6, null, null] },
  ],
};

export const thinking: Thinking[] = [
  { model: 'claude-opus-5-5', effort: 'xhigh', share: 0.64 },
  { model: 'claude-fable-5-1', effort: 'xhigh', share: 0.58 },
  { model: 'claude-opus-5-5', effort: 'high', share: 0.39 },
  { model: 'claude-sonnet-5-5', effort: 'medium', share: 0.12 },
];

export const fallbacks: Fallbacks = {
  pairs: [
    { requested: 'claude-fable-5-1', served: 'claude-opus-5-5', turns: 31, sessions: 2 },
    { requested: 'claude-opus-5-5', served: 'claude-sonnet-5-5', turns: 4, sessions: 1 },
  ],
  rate: 0.004,
};

export const repos: Repos = {
  workspace: { path: '~/gitRepos', marker: '.tokensave-workspace.toml', scanDepth: 1 },
  repos: [
    { name: 'inkcast', path: '~/gitRepos/inkcast', indexed: true, spentTokens: 41.3e6, savedTokens: 6.2e6, tools: ['Claude Code'], lastActivity: at(2) },
    { name: 'service-a', path: '~/gitRepos/service-a', indexed: true, spentTokens: 3.1e6, savedTokens: 0.4e6, tools: ['Claude Code', 'Codex'], lastActivity: at(1440) },
    { name: 'secrets-demo', path: '~/gitRepos/secrets-demo', indexed: true, spentTokens: 1.9e6, savedTokens: 0.2e6, tools: ['Codex'], lastActivity: at(4 * 1440) },
    { name: 'docs-site', path: '~/gitRepos/docs-site', indexed: true, spentTokens: 0.8e6, savedTokens: 0.1e6, tools: ['Claude Code'], lastActivity: at(7 * 1440) },
    { name: 'infra-lab', path: '~/gitRepos/infra-lab', indexed: false, spentTokens: 1.5e6, savedTokens: null, tools: ['Cursor'], lastActivity: at(3 * 1440) },
    { name: 'data-lab', path: '~/gitRepos/data-lab', indexed: false, spentTokens: null, savedTokens: null, tools: [], lastActivity: null },
  ],
};

export const savings: Savings = {
  savedTokens: 6.9e6,
  beforeTokens: 7.8e6,
  afterTokens: 0.9e6,
  graphCalls: 2418,
  missedTokens: 3.4e6,
  byTool: [
    { tool: 'tokensave_context', savedTokens: 2.9e6, share: 0.42 },
    { tool: 'tokensave_search', savedTokens: 1.6e6, share: 0.23 },
    { tool: 'tokensave_impact', savedTokens: 0.9e6, share: 0.13 },
    { tool: 'tokensave_files', savedTokens: 0.7e6, share: 0.1 },
    { tool: 'tokensave_diff', savedTokens: 0.5e6, share: 0.07 },
    { tool: 'other', savedTokens: 0.3e6, share: 0.05 },
  ],
  missed: [
    { kind: 'Whole-file reads', tokens: 2.1e6, turns: 1204, instead: 'outline / read / node' },
    { kind: 'Symbol searches with grep', tokens: 0.9e6, turns: 611, instead: 'search / callers / impact' },
    { kind: 'File discovery with glob', tokens: 0.4e6, turns: 188, instead: 'files / search' },
  ],
};

export const sources: Source[] = [
  { tool: 'Claude Code', path: '~/.claude/projects/', turns: 18402, lastParsed: at(2), status: 'verified', note: 'Model, requested model, effort, thinking, subagent roles.' },
  { tool: 'Codex', path: '~/.codex/sessions/', turns: 2961, lastParsed: at(2), status: 'verified', note: 'One turn per response: model and effort from turn context, tokens from its usage record.' },
  { tool: 'Cursor', path: 'Cursor account (tokensave cursor sync)', turns: 640, lastParsed: at(2), status: 'verified', note: 'Usage per request from your Cursor account; off until you switch it on.' },
  { tool: 'Pi', path: '~/.pi/agent/sessions/', turns: null, lastParsed: null, status: 'not found', note: 'Read by tokensave already; nothing on this machine.' },
  { tool: 'Factory Droid', path: '(as tokensave reads it)', turns: null, lastParsed: null, status: 'not found', note: 'Read by tokensave already; nothing on this machine.' },
];

/** The JSON body for an API path and query, as the tokensave server would answer it. */
export function fixtureFor(pathname: string, query: URLSearchParams): unknown {
  const { end, n } = requestedDays(query);
  const bodies: Record<string, () => unknown> = {
    '/api/status': () => status,
    '/api/summary': () => summary(n),
    '/api/daily': () => daily(end, n),
    '/api/mix': () => mix,
    '/api/runs/top': () => topRuns.slice(0, Number(query.get('limit') ?? topRuns.length)),
    '/api/runs': () => runs,
    '/api/roles': () => roles,
    '/api/models': () => models,
    '/api/thinking': () => thinking,
    '/api/fallbacks': () => fallbacks,
    '/api/repos': () => repos,
    '/api/savings': () => savings,
    '/api/sources': () => sources,
  };
  const body = bodies[pathname];
  if (!body) throw new Error(`no fixture for ${pathname}`);
  return body();
}

/** The same endpoints answering with nothing in them: every section's empty state. */
export function emptyFor(pathname: string): unknown {
  const bodies: Record<string, unknown> = {
    '/api/status': { ...status, workspaces: [] },
    '/api/summary': { spentTokens: 0, costUsd: 0, unpricedTurns: 0, savedTokens: 0, savingRate: 0, cacheHitRate: 0 } satisfies Summary,
    '/api/daily': [] satisfies Day[],
    '/api/mix': { byTool: [], byModel: [] } satisfies Mix,
    '/api/runs/top': [] satisfies TopRun[],
    '/api/runs': [] satisfies Run[],
    '/api/roles': { roles: [], subagentShare: 0, unfinishedRuns: 0 } satisfies RoleCost,
    '/api/models': { efforts: ['low', 'medium', 'high', 'xhigh', 'max'], rows: [] } satisfies ModelEffort,
    '/api/thinking': [] satisfies Thinking[],
    '/api/fallbacks': { pairs: [], rate: 0 } satisfies Fallbacks,
    '/api/repos': { workspace: null, repos: [] } satisfies Repos,
    '/api/savings': { savedTokens: 0, beforeTokens: 0, afterTokens: 0, graphCalls: 0, missedTokens: 0, byTool: [], missed: [] } satisfies Savings,
    '/api/sources': [] satisfies Source[],
  };
  if (!(pathname in bodies)) throw new Error(`no empty fixture for ${pathname}`);
  return bodies[pathname];
}

/** Every endpoint path, for tests that walk them all. */
export const API_PATHS = [
  '/api/status',
  '/api/summary',
  '/api/daily',
  '/api/mix',
  '/api/runs/top',
  '/api/runs',
  '/api/roles',
  '/api/models',
  '/api/thinking',
  '/api/fallbacks',
  '/api/repos',
  '/api/savings',
  '/api/sources',
] as const;
