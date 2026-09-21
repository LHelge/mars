// The side panel's tab registry.
//
// `SidePanel` renders whatever is in this array, in this order, so a new panel
// is one entry here and one component file — `SessionView` never learns its
// name.
//
// A panel takes the session and reaches the socket through
// `useSessionSocketApi()`, so the registry carries no props of its own.

// `TerminalView` is the one entry loaded on demand: xterm and its stylesheet
// are the heaviest thing in the session view and most sessions are read, not
// driven. `SidePanel` renders only the active entry behind a `Suspense`, so the
// chunk is fetched the first time the Terminal tab is opened. A `lazy` result
// is a `ComponentType`, so the registry shape is unchanged.
//
// The order below is therefore not cosmetic. `SidePanel` shows the first
// applicable entry until the operator picks another one, so the Terminal comes
// last: it is the one panel that costs something to show — a lazy chunk and,
// once the session runs, a `/bin/bash -l` exec in the container — and nobody
// gets that without asking for it (`SPEC.md`, "Frontend", "Session side
// panel").

import { lazy } from "react";
import type { ComponentType } from "react";

import type { Session } from "../types";
import { ChangesPanel } from "./ChangesPanel";
import { TasksPanel } from "./TasksPanel";

const TerminalView = lazy(() =>
  import("./TerminalView").then((m) => ({ default: m.TerminalView })),
);

export interface SessionPanelProps {
  session: Session;
}

export interface SidePanelEntry {
  /** Stable across renders; also the tab's test id. */
  id: string;
  label: string;
  component: ComponentType<SessionPanelProps>;
  /** Omitted means always: a panel that cannot apply hides its tab instead of
   *  rendering an excuse. */
  enabled?: (session: Session) => boolean;
}

export const sidePanels: SidePanelEntry[] = [
  {
    id: "changes",
    label: "Changes",
    component: ChangesPanel,
    // A session that has not been created yet has no branch to diff.
    enabled: (session) => session.state !== "creating",
  },
  { id: "tasks", label: "Tasks", component: TasksPanel },
  { id: "terminal", label: "Terminal", component: TerminalView },
];

/** The tabs that apply to this session, in registry order. */
export function panelsFor(
  session: Session,
  panels: SidePanelEntry[] = sidePanels,
): SidePanelEntry[] {
  return panels.filter((panel) => panel.enabled?.(session) ?? true);
}
