// dataset thinking: GET /api/thinking?range&from&to&repo&tool → Thinking[]
import type { Thinking } from '../types';
import { endpoint } from './http';
import { repoAndTool, type Scope } from './params';

export const thinkingApi = endpoint<Thinking[], Scope>('/api/thinking', repoAndTool);
