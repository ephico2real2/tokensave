// dataset savings: GET /api/savings?range&from&to&repo → Savings
import type { Savings } from '../types';
import { endpoint } from './http';
import { repoOnly, type Scope } from './params';

export const savingsApi = endpoint<Savings, Scope>('/api/savings', repoOnly);
