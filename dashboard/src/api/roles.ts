// dataset roles: GET /api/roles?range&from&to&repo&tool&model&effort → RoleCost
import type { RoleCost } from '../types';
import { endpoint } from './http';
import { allFilters, type Scope } from './params';

export const rolesApi = endpoint<RoleCost, Scope>('/api/roles', allFilters);
