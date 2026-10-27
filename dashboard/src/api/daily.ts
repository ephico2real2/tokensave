// dataset daily: GET /api/daily?range&from&to&repo&tool&model&effort → Day[]
import type { Day } from '../types';
import { endpoint } from './http';
import { allFilters, type Scope } from './params';

export const dailyApi = endpoint<Day[], Scope>('/api/daily', allFilters);
