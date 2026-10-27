// dataset models: GET /api/models?range&from&to&repo&tool → ModelEffort
import type { ModelEffort } from '../types';
import { endpoint } from './http';
import { repoAndTool, type Scope } from './params';

export const modelsApi = endpoint<ModelEffort, Scope>('/api/models', repoAndTool);
