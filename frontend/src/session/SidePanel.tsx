// The right-hand column of the session view: one registered panel at a time.
//
// The transcript is the page; the panel is reference material beside it. At
// Tailwind's `lg` there is room for both, so it opens as a column and collapses
// to a rail. Below `lg` a column would squeeze the transcript and the composer
// to nothing, so the panel is a sheet instead: closed, it takes no width at all
// — its opener is the `Panels` button in the session header — and open, it
// covers the lower part of the session box over a dimming overlay, as a dialog
// that Escape, its close button or a tap on the overlay dismisses. One tab
// strip and one tab panel in either chrome; only the frame around them differs.
//
// Crossing the breakpoint — a window resized, a tablet turned — puts the panel
// back where that width wants it: the column's show or hide goes back to the
// width, and a sheet is closed. Between crossings the operator's own show or
// hide stands.
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

import { Suspense, useEffect, useId, useRef, useState } from "react";
import type { KeyboardEvent, ReactNode } from "react";

import { TAP } from "../components/fieldStyles";
import { Icon, ICON_CLASS } from "../components/icons";
import { LoadingState } from "../components/LoadingState";
import { LG_QUERY, useMediaQuery } from "../hooks/useMediaQuery";
import type { Session } from "../types";
import { SessionSheet } from "./SessionSheet";
import { panelOpenerId, setPanelSheet, usePanelSheet } from "./sessionUi";
import { panelsFor } from "./sidePanels";
import type { SidePanelEntry } from "./sidePanels";

export interface SidePanelProps {
  session: Session;
  /** Defaults to the registry; tests and later panels pass their own. */
  panels?: SidePanelEntry[];
}

const CHROME_BUTTON = `text-console-muted hover:text-console-text p-1 ${TAP}`;

export function SidePanel({ session, panels }: SidePanelProps) {
  const entries = panelsFor(session, panels);
  // Below `lg` a column would leave the transcript unreadably narrow, so the
  // panel is a sheet over it instead.
  const wide = useMediaQuery(LG_QUERY);
  /** The operator's show or hide of the column; `null` until they choose, and
   *  after a crossing of the breakpoint, which hands the choice back to the
   *  width. */
  const [choice, setChoice] = useState<boolean | null>(null);
  const [seenWide, setSeenWide] = useState(wide);
  if (seenWide !== wide) {
    setSeenWide(wide);
    setChoice(null);
  }
  const open = choice ?? wide;
  // The sheet's flag is shared with the header's opener, so it lives in the
  // per-session UI store. A crossing of `lg`, and a fresh mount, closes it:
  // nothing opened on one side of the breakpoint shows up on the other.
  const sheetFlag = usePanelSheet(session.id);
  useEffect(() => {
    setPanelSheet(session.id, false);
  }, [wide, session.id]);
  const sheet = !wide && sheetFlag;

  /** The tab the operator selected; `null` until they select one. */
  const [activeId, setActiveId] = useState<string | null>(null);
  const tabs = useRef(new Map<string, HTMLButtonElement>());
  const dialog = useRef<HTMLDivElement>(null);
  const uid = useId();

  // A dialog takes the focus when it opens: onto the selected tab, the strip's
  // one tab stop.
  useEffect(() => {
    if (!sheet) return;
    dialog.current
      ?.querySelector<HTMLElement>('[role="tab"][aria-selected="true"]')
      ?.focus();
  }, [sheet]);

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

  /** Closes the sheet and hands the focus back to the button that opened it. */
  const closeSheet = (): void => {
    setPanelSheet(session.id, false);
    document.getElementById(panelOpenerId(session.id))?.focus();
  };

  /** The strip: the tablist, then the chrome's own close control. */
  const strip = (close: ReactNode): ReactNode => (
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
            className={`border-b-2 px-3 py-2 font-mono text-xs ${TAP} ${
              entry.id === active.id
                ? "border-console-accent text-console-text"
                : "text-console-muted hover:text-console-text border-transparent"
            }`}
          >
            {entry.label}
          </button>
        ))}
      </div>
      {close}
    </div>
  );

  // Only the active entry is rendered, so a panel loaded on demand (the
  // Terminal, whose xterm chunk is fetched when its tab is first opened)
  // suspends here and nowhere else.
  const tabPanel = (
    <div
      role="tabpanel"
      id={panelId}
      aria-labelledby={tabId(active.id)}
      className="min-h-0 flex-1 overflow-y-auto overscroll-contain"
    >
      <Suspense fallback={<LoadingState label="Loading panel" />}>
        <Panel session={session} />
      </Suspense>
    </div>
  );

  if (!wide) {
    // Closed, the sheet takes no room: its opener is the header's `Panels`.
    if (!sheet) return null;
    return (
      <SessionSheet ref={dialog} label="Session panels" onClose={closeSheet}>
        {strip(
          <button
            type="button"
            aria-label="Close panels"
            onClick={closeSheet}
            className={CHROME_BUTTON}
          >
            <Icon.close aria-hidden="true" className={ICON_CLASS} />
          </button>,
        )}
        {tabPanel}
      </SessionSheet>
    );
  }

  if (!open) {
    return (
      <aside className="border-console-border flex shrink-0 flex-col items-center gap-2 border-l px-1 py-2">
        <button
          type="button"
          aria-label="Show side panel"
          onClick={() => {
            setChoice(true);
          }}
          className={CHROME_BUTTON}
        >
          <Icon.panelOpen aria-hidden="true" className={ICON_CLASS} />
        </button>
      </aside>
    );
  }

  return (
    <aside className="border-console-border flex w-96 max-w-full shrink-0 flex-col border-l">
      {strip(
        <button
          type="button"
          aria-label="Hide side panel"
          onClick={() => {
            setChoice(false);
          }}
          className={CHROME_BUTTON}
        >
          <Icon.panelClose aria-hidden="true" className={ICON_CLASS} />
        </button>,
      )}
      {tabPanel}
    </aside>
  );
}
