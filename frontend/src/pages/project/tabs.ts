// The tab registry of `/projects/:id` (`SPEC.md`, "Frontend", Routes:
// sessions, board, profiles, shared directories, states and secrets).
//
// The selection is the `?tab=` search parameter, so a tab is a real link: it
// survives a reload, can be shared and can be opened in a new tab. A panel
// that takes the whole project takes `ProjectTabPanelProps` and nothing else;
// the secrets and states tabs mount shared components with narrower props. No
// panel re-reads the project.
//
// It lives beside `ProjectTabs.tsx` rather than in it because a module that
// renders a component exports nothing else (`react-refresh/only-export-
// components`); the `./project` barrel hands out both.

import type { Project } from "../../types";

export type ProjectTab =
  | "sessions"
  | "board"
  | "profiles"
  | "shared-dirs"
  | "states"
  | "secrets";

export const DEFAULT_PROJECT_TAB: ProjectTab = "sessions";

export const PROJECT_TABS: { value: ProjectTab; label: string }[] = [
  { value: "sessions", label: "Sessions" },
  { value: "board", label: "Board" },
  { value: "profiles", label: "Profiles" },
  { value: "shared-dirs", label: "Shared directories" },
  { value: "states", label: "States" },
  { value: "secrets", label: "Secrets" },
];

/** Every panel gets the project the page already loaded; none re-fetches it. */
export interface ProjectTabPanelProps {
  project: Project;
}

/** An unknown or missing `?tab=` value falls back to the sessions tab. */
export function parseProjectTab(raw: string | null): ProjectTab {
  const match = PROJECT_TABS.find((entry) => entry.value === raw);
  return match?.value ?? DEFAULT_PROJECT_TAB;
}
