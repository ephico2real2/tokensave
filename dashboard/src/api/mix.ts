// dataset mix: GET /api/mix?range&from&to&repo&tool&model&effort → Mix
import type { Mix } from '../types';
import { endpoint } from './http';
import { allFilters, type Scope } from './params';

export const mixApi = endpoint<Mix, Scope>('/api/mix', allFilters);
