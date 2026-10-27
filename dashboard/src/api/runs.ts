// dataset runs: GET /api/runs?range&from&to&repo&tool&model&effort → Run[]
import type { Run } from '../types';
import { endpoint } from './http';
import { allFilters, type Scope } from './params';

export const runsApi = endpoint<Run[], Scope>('/api/runs', allFilters);
