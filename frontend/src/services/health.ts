// Temporary: the Frontend foundation epic replaces this with `apiClient.ts`.
// Components never call `fetch`; every request goes through `services/`.

export interface Health {
  orchestrator: boolean;
  database: boolean;
  engine: boolean;
}

export async function getHealth(): Promise<Health> {
  const response = await fetch("/api/health");
  if (!response.ok) {
    throw new Error(`health check failed with ${String(response.status)}`);
  }
  return (await response.json()) as Health;
}
