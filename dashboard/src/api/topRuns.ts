// dataset topRuns: GET /api/runs/top?limit=5&range&from&to&repo&tool&model&effort → TopRun[]
import type { TopRun } from '../types';
import { endpoint } from './http';
import { allFilters, type Scope } from './params';

/** How many runs the Overview lists. */
export const TOP_RUNS = 5;

export const topRunsApi = endpoint<TopRun[], Scope>('/api/runs/top', (scope) => ({ limit: TOP_RUNS, ...allFilters(scope) }));
