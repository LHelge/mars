// One badge for the lifecycle state of a session (`SessionState`), as the
// dashboard and the session lists show it. Only the four state tokens carry
// colour (`CLAUDE.md`, "Frontend conventions": quiet colour reserved for
// state); `done` is neutral, so a list of finished work reads as quiet as it
// is. A project's clone state is a different vocabulary and is drawn by
// `pages/projects/ProjectStatusPill`, which colours its `error` red.

import type { SessionState } from "../types";

export interface StatusBadgeProps {
  state: SessionState;
}

const COLOURS: Record<SessionState, string> = {
  running: "text-state-running",
  parked: "text-state-parked",
  failed: "text-state-failed",
  done: "text-console-muted",
  creating: "text-console-muted",
};

const LABELS: Record<SessionState, string> = {
  running: "running",
  parked: "parked",
  failed: "failed",
  done: "done",
  creating: "creating",
};

export function StatusBadge({ state }: StatusBadgeProps) {
  return (
    <span
      className={`border-console-border bg-console-surface inline-flex items-center gap-1.5 rounded border px-1.5 py-0.5 font-mono text-xs ${COLOURS[state]}`}
    >
      <span aria-hidden="true" className="size-1.5 rounded-full bg-current" />
      {LABELS[state]}
    </span>
  );
}
