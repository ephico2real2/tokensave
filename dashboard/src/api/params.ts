// The query each endpoint takes, exactly as its `// dataset` line in the mockup lists it.

import { rangeQuery, type UrlState } from '../state';
import type { Query } from './http';

export type Scope = Pick<UrlState, 'range' | 'from' | 'to' | 'workspace' | 'repo' | 'tool' | 'model' | 'effort'>;

/** ?range&from&to&repo&tool&model&effort */
export const allFilters = (s: Scope): Query => ({
  ...rangeQuery(s),
  repo: s.repo,
  tool: s.tool,
  model: s.model,
  effort: s.effort,
});

/** ?range&from&to&repo&tool */
export const repoAndTool = (s: Scope): Query => ({ ...rangeQuery(s), repo: s.repo, tool: s.tool });

/** ?workspace&range&from&to&tool */
export const workspaceAndTool = (s: Scope): Query => ({ workspace: s.workspace, ...rangeQuery(s), tool: s.tool });

/** ?range&from&to&repo */
export const repoOnly = (s: Scope): Query => ({ ...rangeQuery(s), repo: s.repo });
