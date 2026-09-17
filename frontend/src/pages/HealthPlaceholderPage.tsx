import { useQuery } from "@tanstack/react-query";
import { getHealth } from "../services/health";
import type { Health } from "../services/health";

export function HealthPlaceholderPage() {
  const { data, isPending, isError } = useQuery<Health>({
    queryKey: ["health"],
    queryFn: getHealth,
    retry: false,
  });

  return (
    <main className="mx-auto flex min-h-full max-w-2xl flex-col gap-4 p-8">
      <h1 className="text-lg font-semibold tracking-tight">Mars</h1>
      <p className="text-console-muted">
        Placeholder route. Pages, services and stores arrive with the frontend
        epics.
      </p>
      <section
        aria-label="Orchestrator health"
        className="border-console-border bg-console-surface rounded border p-4"
      >
        {isPending && <p>checking orchestrator…</p>}
        {isError && <p className="text-state-failed">orchestrator unreachable</p>}
        {data && (
          <ul className="space-y-1">
            <li>orchestrator: {String(data.orchestrator)}</li>
            <li>database: {String(data.database)}</li>
            <li>engine: {String(data.engine)}</li>
          </ul>
        )}
      </section>
    </main>
  );
}
