// One badge for every state the dashboard, the admin page and the session and
// task lists show. Only the four state tokens carry colour (`CLAUDE.md`,
// "Frontend conventions": quiet colour reserved for state); the remaining
// states are neutral, so a list of finished work reads as quiet as it is.

export type BadgeState =
  | "running"
  | "parked"
  | "failed"
  | "human"
  | "done"
  | "creating"
  | "cloning"
  | "ready"
  | "error";

export interface StatusBadgeProps {
  state: BadgeState;
  /** Overrides the state's own name. */
  label?: string;
}

const COLOURS: Record<BadgeState, string> = {
  running: "text-state-running",
  parked: "text-state-parked",
  failed: "text-state-failed",
  human: "text-state-human",
  done: "text-console-muted",
  creating: "text-console-muted",
  cloning: "text-console-muted",
  ready: "text-console-muted",
  error: "text-console-muted",
};

const LABELS: Record<BadgeState, string> = {
  running: "running",
  parked: "parked",
  failed: "failed",
  human: "needs human",
  done: "done",
  creating: "creating",
  cloning: "cloning",
  ready: "ready",
  error: "error",
};

export function StatusBadge({ state, label }: StatusBadgeProps) {
  return (
    <span
      className={`border-console-border bg-console-surface inline-flex items-center gap-1.5 rounded border px-1.5 py-0.5 font-mono text-xs ${COLOURS[state]}`}
    >
      <span
        aria-hidden="true"
        className="size-1.5 rounded-full bg-current"
      />
      {label ?? LABELS[state]}
    </span>
  );
}
