// The tab strip of `/projects/:id`. The entries themselves are in `./tabs`;
// this is only their presentation: one row of links, each carrying its own
// `?tab=` value, the current one marked as the current page. Below `sm` the row
// wraps, so a phone sees every tab in two rows rather than a strip that
// scrolls with no cue; from `sm` it is one row, scrolling only as a safety net.

import { Link } from "react-router";
import { TAP } from "../../components/fieldStyles";
import { PROJECT_TABS } from "./tabs";
import type { ProjectTab } from "./tabs";

export interface ProjectTabsProps {
  projectId: string;
  active: ProjectTab;
}

function tabClass(isActive: boolean): string {
  return [
    // A flex box on touch, so the 44 px tab keeps its label centred.
    `shrink-0 border-b-2 px-1 py-2 text-sm whitespace-nowrap pointer-coarse:inline-flex pointer-coarse:items-center ${TAP}`,
    isActive
      ? "border-console-accent text-console-text"
      : "border-transparent text-console-muted hover:text-console-text",
  ].join(" ");
}

export function ProjectTabs({ projectId, active }: ProjectTabsProps) {
  return (
    <nav
      aria-label="Project sections"
      className="border-console-border flex min-w-0 items-center gap-x-4 gap-y-0 overflow-x-auto border-b max-sm:flex-wrap"
    >
      {PROJECT_TABS.map((entry) => (
        <Link
          key={entry.value}
          to={`/projects/${projectId}?tab=${entry.value}`}
          // The tab strip is navigation, so the current tab is the current
          // page rather than a pressed control.
          aria-current={entry.value === active ? "page" : undefined}
          className={tabClass(entry.value === active)}
        >
          {entry.label}
        </Link>
      ))}
    </nav>
  );
}
