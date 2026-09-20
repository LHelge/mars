// The clone state of a project (`docs/data-model.md`, `projects.status`), as
// the one coloured thing on a projects row. `cloning` pulses because work is
// happening, `ready` is deliberately quiet, `error` is the only red on the
// page. The `status_message` that comes with an `error` belongs to the row,
// not to the pill: the pill stays one line wide wherever it is used.

import type { ProjectStatus } from "../../types";

const STYLES: Record<ProjectStatus, string> = {
  cloning: "text-console-muted",
  ready: "text-console-muted",
  error: "text-state-failed border-state-failed/60",
};

export function ProjectStatusPill({ status }: { status: ProjectStatus }) {
  return (
    <span
      className={`border-console-border bg-console-surface inline-flex items-center gap-1.5 rounded border px-1.5 py-0.5 font-mono text-xs whitespace-nowrap ${STYLES[status]}`}
    >
      <span
        aria-hidden="true"
        className={`size-1.5 rounded-full bg-current ${status === "cloning" ? "animate-pulse" : ""}`}
      />
      {status}
    </span>
  );
}
