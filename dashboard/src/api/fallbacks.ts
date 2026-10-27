// dataset fallbacks: GET /api/fallbacks?range&from&to&repo&tool → Fallbacks
import type { Fallbacks } from '../types';
import { endpoint } from './http';
import { repoAndTool, type Scope } from './params';

export const fallbacksApi = endpoint<Fallbacks, Scope>('/api/fallbacks', repoAndTool);
