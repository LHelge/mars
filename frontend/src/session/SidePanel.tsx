// The right-hand column of the session view: one registered panel at a time.
//
// The transcript is the page; the panel is reference material beside it. On a
// wide screen there is room for both, so it opens; on a narrow one it starts
// collapsed to a rail and the transcript keeps the width.
//
// Which tab is shown is *derived*, not initialised: the state is the operator's
// own choice, `null` until they click or arrow onto a tab, and the tab on
// screen is that choice when it still applies and otherwise the first
// applicable entry. A session launched from the UI arrives here `creating`,
// where `Changes` has no branch to diff and is not offered yet; with a captured
// initial tab it would open on whatever happened to be first at that instant
// and stay there, which is how every launched session used to be handed a
// terminal nobody asked for. Derived, it follows the registry: `Tasks` while
// the session is being created and `Changes` from the moment there is one
// (`SPEC.md`, "Frontend", "Session side panel").

import { Suspense, useId, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
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
  /** The tab the operator selected; `null` until they select one. */
  const [activeId, setActiveId] = useState<string | null>(null);
  const tabs = useRef(new Map<string, HTMLButtonElement>());
  const uid = useId();

  // The selected tab, or the first one. `undefined` is the empty case — this
  // session offers no panel at all — and renders nothing, as it always did.
  const active = entries.find((entry) => entry.id === activeId) ?? entries[0];
  if (active === undefined) {
    return null;
  }

  const Panel = active.component;
  const tabId = (id: string): string => `${uid}-tab-${id}`;
  const panelId = `${uid}-panel`;

  /** The tablist's own keys: arrows move and select, Home and End jump. */
  const onTabKeyDown = (event: KeyboardEvent<HTMLDivElement>): void => {
    const at = entries.indexOf(active);
    const next =
      event.key === "ArrowRight"
        ? entries[(at + 1) % entries.length]
        : event.key === "ArrowLeft"
          ? entries[(at - 1 + entries.length) % entries.length]
          : event.key === "Home"
            ? entries[0]
            : event.key === "End"
              ? entries[entries.length - 1]
              : undefined;
    if (next === undefined) return;
    event.preventDefault();
    setActiveId(next.id);
    tabs.current.get(next.id)?.focus();
  };

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
        {/* One tab stop for the whole list, moved between tabs with the arrow
            keys: the roving `tabIndex` of the ARIA tabs pattern. */}
        <div
          role="tablist"
          aria-label="Session panels"
          className="flex min-w-0 flex-1"
          onKeyDown={onTabKeyDown}
        >
          {entries.map((entry) => (
            <button
              key={entry.id}
              type="button"
              role="tab"
              id={tabId(entry.id)}
              aria-selected={entry.id === active.id}
              aria-controls={panelId}
              tabIndex={entry.id === active.id ? 0 : -1}
              ref={(node) => {
                if (node === null) {
                  tabs.current.delete(entry.id);
                } else {
                  tabs.current.set(entry.id, node);
                }
              }}
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
      <div
        role="tabpanel"
        id={panelId}
        aria-labelledby={tabId(active.id)}
        className="min-h-0 flex-1 overflow-y-auto"
      >
        <Suspense fallback={<LoadingState label="Loading panel" />}>
          <Panel session={session} />
        </Suspense>
      </div>
    </aside>
  );
}
