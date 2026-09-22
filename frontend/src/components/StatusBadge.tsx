// One badge for the lifecycle state of a session (`SessionState`), as the
// dashboard and the session lists show it. Only `running`, `parked` and
// `failed` carry colour (`CLAUDE.md`, "Frontend conventions": quiet colour
// reserved for state); `creating` and `done` are neutral, so a list of
// finished work reads as quiet as it is. A project's clone state is a different vocabulary and is drawn by
// `pages/projects/ProjectStatusPill`, which colours its `error` red.

import type { SessionState } from "../types";
import { Icon, ICON_CLASS } from "./icons";
import type { IconComponent } from "./icons";

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

/** Beside the label, never instead of it: the shape tells states apart where
 * colour alone would not. */
const ICONS: Record<SessionState, IconComponent> = {
  running: Icon.running,
  parked: Icon.parked,
  failed: Icon.failed,
  done: Icon.done,
  creating: Icon.creating,
};

const LABELS: Record<SessionState, string> = {
  running: "running",
  parked: "parked",
  failed: "failed",
  done: "done",
  creating: "creating",
};

export function StatusBadge({ state }: StatusBadgeProps) {
  const Glyph = ICONS[state];
  return (
    <span
      className={`border-console-border bg-console-surface inline-flex items-center gap-1.5 rounded border px-1.5 py-0.5 font-mono text-xs ${COLOURS[state]}`}
    >
      <Glyph aria-hidden="true" className={ICON_CLASS} />
      {LABELS[state]}
    </span>
  );
}
