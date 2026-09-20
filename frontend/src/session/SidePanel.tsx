// The right-hand column of the session view: one registered panel at a time.
//
// The transcript is the page; the panel is reference material beside it. On a
// wide screen there is room for both, so it opens; on a narrow one it starts
// collapsed to a rail and the transcript keeps the width.

import { Suspense, useState } from "react";
import {
  ChevronDoubleLeftIcon,
  ChevronDoubleRightIcon,
} from "@heroicons/react/24/outline";

import { LoadingState } from "../components/LoadingState";
import type { Session } from "../types";
import { panelsFor } from "./sidePanels";
import type { SidePanelEntry } from "./sidePanels";

/** Below this the panel would leave the transcript unreadably narrow. */
const WIDE_PX = 1024;

function wideScreen(): boolean {
  return (
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia(`(min-width: ${String(WIDE_PX)}px)`).matches
  );
}

export interface SidePanelProps {
  session: Session;
  /** Defaults to the registry; tests and later panels pass their own. */
  panels?: SidePanelEntry[];
}

export function SidePanel({ session, panels }: SidePanelProps) {
  const entries = panelsFor(session, panels);
  const [open, setOpen] = useState(wideScreen);
  const [activeId, setActiveId] = useState(entries[0]?.id);

  if (entries.length === 0) {
    return null;
  }

  const active = entries.find((entry) => entry.id === activeId) ?? entries[0];
  const Panel = active.component;

  if (!open) {
    return (
      <aside className="border-console-border flex shrink-0 flex-col items-center gap-2 border-l px-1 py-2">
        <button
          type="button"
          aria-label="Show side panel"
          onClick={() => {
            setOpen(true);
          }}
          className="text-console-muted hover:text-console-text p-1"
        >
          <ChevronDoubleLeftIcon aria-hidden="true" className="size-4" />
        </button>
      </aside>
    );
  }

  return (
    <aside className="border-console-border flex w-96 max-w-full shrink-0 flex-col border-l">
      <div className="border-console-border flex items-center gap-1 border-b px-1">
        <div role="tablist" aria-label="Session panels" className="flex min-w-0 flex-1">
          {entries.map((entry) => (
            <button
              key={entry.id}
              type="button"
              role="tab"
              aria-selected={entry.id === active.id}
              onClick={() => {
                setActiveId(entry.id);
              }}
              className={`border-b-2 px-3 py-2 font-mono text-xs ${
                entry.id === active.id
                  ? "border-console-accent text-console-text"
                  : "border-transparent text-console-muted hover:text-console-text"
              }`}
            >
              {entry.label}
            </button>
          ))}
        </div>
        <button
          type="button"
          aria-label="Hide side panel"
          onClick={() => {
            setOpen(false);
          }}
          className="text-console-muted hover:text-console-text p-1"
        >
          <ChevronDoubleRightIcon aria-hidden="true" className="size-4" />
        </button>
      </div>

      {/* Only the active entry is rendered, so a panel loaded on demand (the
          Terminal, whose xterm chunk is fetched when its tab is first opened)
          suspends here and nowhere else. */}
      <div role="tabpanel" className="min-h-0 flex-1 overflow-y-auto">
        <Suspense fallback={<LoadingState label="Loading panel" />}>
          <Panel session={session} />
        </Suspense>
      </div>
    </aside>
  );
}
