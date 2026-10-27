// The API contract: one type per `// type` line of the mockup's datasets
// (inkcast docs/tooling/tokensave-dashboard/mockups/source/Main.dc.html), shared by the
// fetch modules, the components and the test fixtures. Numbers are raw (tokens, USD, shares
// in 0..1); times are ISO 8601; dates are YYYY-MM-DD. format.ts turns them into text.

export type Tool = 'Claude Code' | 'Codex' | 'Cursor' | 'Pi' | 'Factory Droid';
export type Role = 'main' | 'builder' | 'reviewer' | 'Explore' | 'other';
export type Effort = 'low' | 'medium' | 'high' | 'xhigh' | 'max';

export const TOOLS: readonly Tool[] = ['Claude Code', 'Codex', 'Cursor', 'Pi', 'Factory Droid'];
export const EFFORTS: readonly Effort[] = ['low', 'medium', 'high', 'xhigh', 'max'];

/** GET /api/status */
export type Status = { bind: string; port: number; workspaces: string[] };

/** GET /api/summary?range&from&to&repo&tool&model&effort */
export type Summary = {
  spentTokens: number;
  costUsd: number;
  unpricedTurns: number;
  savedTokens: number;
  savingRate: number;
  cacheHitRate: number;
};

/** GET /api/daily?range&from&to&repo&tool&model&effort: one per day of the range. */
export type Day = { date: string; spentTokens: number; savedTokens: number };

/** GET /api/mix?range&from&to&repo&tool&model&effort */
export type Mix = {
  byTool: { tool: Tool; tokens: number; share: number }[];
  byModel: { model: string; tokens: number; share: number }[];
};

/** GET /api/runs/top?limit=5&range&from&to&repo&tool&model&effort */
export type TopRun = {
  id: string;
  name: string;
  role: Role;
  repo: string;
  model: string;
  effort: Effort | null;
  tokens: number;
  costUsd: number | null;
};

export type RunState = 'finished' | 'running' | 'stopped' | 'partial';

/** GET /api/runs?range&from&to&repo&tool&model&effort */
export type Run = {
  id: string;
  parentId: string | null;
  name: string;
  role: Role;
  tool: Tool;
  model: string;
  effort: Effort | null;
  inputTokens: number;
  outputTokens: number | null;
  costUsd: number | null;
  state: RunState;
};

/** GET /api/roles?range&from&to&repo&tool&model&effort */
export type RoleCost = {
  roles: { role: Role; costUsd: number; share: number }[];
  subagentShare: number;
  unfinishedRuns: number;
};

/** GET /api/models?range&from&to&repo&tool: one cost per effort, null when no turns. */
export type ModelEffort = { efforts: Effort[]; rows: { model: string; costUsd: (number | null)[] }[] };

/** GET /api/thinking?range&from&to&repo&tool: share = thinking tokens ÷ output tokens. */
export type Thinking = { model: string; effort: Effort | null; share: number };

/** GET /api/fallbacks?range&from&to&repo&tool */
export type Fallbacks = {
  pairs: { requested: string; served: string; turns: number; sessions: number }[];
  rate: number;
};

/**
 * GET /api/repos?workspace&range&from&to&tool
 *
 * `workspace` is nullable here although the mockup's type is not: S3a returns null until
 * workspaces exist (S2), and the page shows that as the stat strip's empty state.
 */
export type Repos = {
  workspace: { path: string; marker: string; scanDepth: number } | null;
  repos: Repo[];
};

export type Repo = {
  name: string;
  path: string;
  indexed: boolean;
  spentTokens: number | null;
  savedTokens: number | null;
  tools: Tool[];
  lastActivity: string | null;
};

/** GET /api/savings?range&from&to&repo */
export type Savings = {
  savedTokens: number;
  beforeTokens: number;
  afterTokens: number;
  graphCalls: number;
  missedTokens: number;
  byTool: { tool: string; savedTokens: number; share: number }[];
  missed: { kind: string; tokens: number; turns: number; instead: string }[];
};

export type SourceStatus = 'verified' | 'partial' | 'not found' | 'unknown';

/** GET /api/sources */
export type Source = {
  tool: Tool;
  path: string;
  turns: number | null;
  lastParsed: string | null;
  status: SourceStatus;
  note: string;
};
