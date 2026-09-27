// The UI state a session has beside its transcript: which rows the reader has
// opened, the message a `Resend` handed to the composer, and whether the side
// panel's sheet is open below `lg` (`SPEC.md`, "Frontend", "Transcript
// rendering", "Session store lifecycle" and "Session side panel").
//
// It is here rather than inside the rows because the transcript is virtualised.
// A row a few screens out of view is unmounted, and a `useState` inside it goes
// with it: the reader opened a subagent group or a long tool result, scrolled
// away and came back to a collapsed row — while the virtualizer's size cache,
// which is keyed by message id and survives the unmount, still held the open
// row's height, so the layout jumped as soon as the collapsed row was measured.
// Keyed by message id and held outside the row, both halves stay put.
//
// One store for every session, keyed by session id, so that the seam the
// registry offers — `onSessionStoreCleared`, which fires on every clear,
// eviction and dispose — is all it takes to end a session's UI state with its
// transcript. No component lives in this module: a file that renders one may
// export nothing else (`react-refresh/only-export-components`).

import { createContext, useCallback, useContext, useState } from "react";
import { createStore, useStore } from "zustand";

import { onSessionStoreCleared } from "./sessionStore";

/** The transcript's latest `Resend`, which the composer takes by its token. */
export interface ResendRequest {
  text: string;
  /**
   * Monotonic across the application. The composer adopts the text once per
   * token, so resending the same message twice is two requests and not one.
   */
  token: number;
}

interface SessionUi {
  /**
   * Disclosure key -> whether it is open. An absent key is the disclosure's
   * own default, so nothing has to be written down for a row nobody touched.
   */
  expanded: Record<string, boolean>;
  resend: ResendRequest | null;
  /**
   * Whether the side panel is open as a sheet over the transcript. Only a
   * narrow screen has a sheet; its opener is in the session header and the
   * sheet in `SidePanel`, so the one flag both read lives here.
   */
  panelSheet: boolean;
}

interface SessionUiState {
  sessions: Record<string, SessionUi>;
}

const EMPTY: SessionUi = { expanded: {}, resend: null, panelSheet: false };

const uiStore = createStore<SessionUiState>()(() => ({ sessions: {} }));

function update(sessionId: string, change: (ui: SessionUi) => SessionUi): void {
  uiStore.setState((state) => ({
    sessions: {
      ...state.sessions,
      [sessionId]: change(state.sessions[sessionId] ?? EMPTY),
    },
  }));
}

/**
 * The session a transcript row belongs to. The rows below `Transcript` are
 * reached through a tool renderer registry that hands a component nothing but
 * its message, so the id that keys this state is published here rather than
 * threaded through every renderer's props.
 */
export const SessionUiContext = createContext<string | null>(null);

/** The key one disclosure of one row is held under; a row may have several. */
export function disclosureKey(rowId: string, slot: string): string {
  return `${rowId}#${slot}`;
}

/** Opens what is closed and closes what is open, from wherever it is called. */
export function toggleDisclosure(
  sessionId: string,
  key: string,
  defaultOpen = false,
): void {
  update(sessionId, (ui) => ({
    ...ui,
    expanded: { ...ui.expanded, [key]: !(ui.expanded[key] ?? defaultOpen) },
  }));
}

/**
 * Whether one disclosure is open, and the handler that flips it.
 *
 * The subscription is per key and returns a boolean, so opening one row
 * re-renders that row alone — the rule a `text_delta` already follows
 * (`SPEC.md`, "Transcript rendering"). Outside a transcript — a component
 * rendered on its own, a unit test — there is no session to key the state to
 * and it stays where it always was, in the component.
 */
export function useDisclosure(
  key: string,
  defaultOpen = false,
): readonly [boolean, () => void] {
  const sessionId = useContext(SessionUiContext);
  const [local, setLocal] = useState(defaultOpen);
  const stored = useStore(uiStore, (state) =>
    sessionId === null ? undefined : state.sessions[sessionId]?.expanded[key],
  );
  const toggle = useCallback(() => {
    if (sessionId === null) {
      setLocal((value) => !value);
      return;
    }
    toggleDisclosure(sessionId, key, defaultOpen);
  }, [defaultOpen, key, sessionId]);

  return [sessionId === null ? local : (stored ?? defaultOpen), toggle];
}

let nextToken = 0;

/**
 * The transcript's `Resend`: the refused message's text, for the composer to
 * put back in the box. A plain call from the button's own handler — nothing is
 * lifted into the view above it and nothing is handed back down as a prop.
 */
export function requestResend(sessionId: string, text: string): void {
  nextToken += 1;
  update(sessionId, (ui) => ({ ...ui, resend: { text, token: nextToken } }));
}

/** The latest `Resend` of this session, or `null` when there has been none. */
export function useResendRequest(sessionId: string): ResendRequest | null {
  return useStore(
    uiStore,
    (state) => state.sessions[sessionId]?.resend ?? null,
  );
}

/** Opens or closes the side panel's sheet; a no-op when it already is. */
export function setPanelSheet(sessionId: string, open: boolean): void {
  if ((uiStore.getState().sessions[sessionId]?.panelSheet ?? false) === open) {
    return;
  }
  update(sessionId, (ui) => ({ ...ui, panelSheet: open }));
}

/** Whether this session's side panel sheet is open. */
export function usePanelSheet(sessionId: string): boolean {
  return useStore(
    uiStore,
    (state) => state.sessions[sessionId]?.panelSheet ?? false,
  );
}

/**
 * The DOM id of the header's `Panels` button, where focus goes back when the
 * sheet closes. One session view per page, but keyed anyway so the id says
 * whose opener it is.
 */
export function panelOpenerId(sessionId: string): string {
  return `panels-opener-${sessionId}`;
}

/** Test-only: what is held for one session, if anything. */
export function peekSessionUi(sessionId: string): SessionUi | undefined {
  return uiStore.getState().sessions[sessionId];
}

// A transcript's UI state is that transcript's: when the store it describes is
// cleared, evicted or disposed, what the reader had opened in it describes
// nothing (`SPEC.md`, "Session store lifecycle"). Registered at import, beside
// the state it acts on, exactly as the registry's own sign-out listener is.
onSessionStoreCleared((sessionId) => {
  if (uiStore.getState().sessions[sessionId] === undefined) return;
  uiStore.setState((state) => {
    const sessions = { ...state.sessions };
    delete sessions[sessionId];
    return { sessions };
  });
});
