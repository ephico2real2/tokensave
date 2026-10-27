// dataset sources: GET /api/sources → Source[]
import type { Source } from '../types';
import { endpoint } from './http';

export const sourcesApi = endpoint<Source[]>('/api/sources');
