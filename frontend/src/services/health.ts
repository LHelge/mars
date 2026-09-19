// `GET /api/health` — unauthenticated, like the invite lookup; every other
// request carries the bearer token.

import { apiGet } from "./apiClient";

export interface Health {
  orchestrator: boolean;
  database: boolean;
  engine: boolean;
}

export function getHealth(): Promise<Health> {
  return apiGet<Health>("/health");
}
