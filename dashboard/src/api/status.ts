// dataset status: GET /api/status → Status
import type { Status } from '../types';
import { endpoint } from './http';

export const statusApi = endpoint<Status>('/api/status');
