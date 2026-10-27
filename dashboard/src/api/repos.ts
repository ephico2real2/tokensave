// dataset repos: GET /api/repos?workspace&range&from&to&tool → Repos
import type { Repos } from '../types';
import { endpoint } from './http';
import { workspaceAndTool, type Scope } from './params';

export const reposApi = endpoint<Repos, Scope>('/api/repos', workspaceAndTool);
