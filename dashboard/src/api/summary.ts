// dataset summary: GET /api/summary?range&from&to&repo&tool&model&effort → Summary
import type { Summary } from '../types';
import { endpoint } from './http';
import { allFilters, type Scope } from './params';

export const summaryApi = endpoint<Summary, Scope>('/api/summary', allFilters);
