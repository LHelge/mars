// The side panel's tab registry.
//
// `SidePanel` renders whatever is in this array, in this order, so a new panel
// is one entry here and one component file — `SessionView` never learns its
// name. The `Changes` and `Terminal` panels are separate tasks and add their
// entries above `Tasks`.
//
// A panel takes the session and reaches the socket through
// `useSessionSocketApi()`, so the registry carries no props of its own.

import type { ComponentType } from "react";

import type { Session } from "../types";
import { TasksPanel } from "./TasksPanel";

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
  { id: "tasks", label: "Tasks", component: TasksPanel },
];

/** The tabs that apply to this session, in registry order. */
export function panelsFor(
  session: Session,
  panels: SidePanelEntry[] = sidePanels,
): SidePanelEntry[] {
  return panels.filter((panel) => panel.enabled?.(session) ?? true);
}
