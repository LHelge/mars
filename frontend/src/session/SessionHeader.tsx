// The session's identity strip.
//
// One dense band above the transcript, read left to right: what state the
// session is in, what it is called, where it is running, what it has cost. The
// only colour is the state pill and the connection dot — everything a session
// can tell you at a glance is a state, and everything else is an identifier, so
// the rest of the strip is monospace and quiet (`CLAUDE.md`, "Frontend
// conventions").
//
// The title is edited in place: it is the one field of a session a user owns,
// and a separate form for one string would be ceremony. Enter and blur commit,
// Escape and the `×` beside the field restore.
//
// Below `sm` the strip folds (`SPEC.md`, "Frontend", "Session header"): one row
// of state, connection and title, and a `Details` disclosure over everything
// else, so a phone's transcript is not pushed off its own screen by the header
// above it.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useId, useRef, useState } from "react";
import { Link } from "react-router";

import { Alert } from "../components/Alert";
import { CopyLinkButton } from "../components/CopyLinkButton";
import { SessionStatePill } from "../components/SessionStatePill";
import { GitActionsPanel } from "../components/git/GitActionsPanel";
import { errorMessage, logUnexpected } from "../services/errorMessage";
import { queryKeys } from "../services/queryKeys";
import { projectQueries } from "../services/queryOptions";
import { updateSession } from "../services/sessions";
import type { Session } from "../types";
import {
  COST_DECIMALS,
  formatRelative,
  formatTokens,
  formatUsd,
  PLACEHOLDER,
  shortId,
  shortSha,
} from "../utils/format";
import { LaunchSourceTag } from "./LaunchSourceTag";
import { SessionActions } from "./SessionActions";
import type { ConnectionStatus, SessionStore } from "./sessionStore";
import { getSessionStore, useSessionStore } from "./sessionStore";
import { panelOpenerId, setPanelSheet, usePanelSheet } from "./sessionUi";
import { useSyncSession } from "./useSyncSession";
import { Icon, ICON_CLASS } from "../components/icons";
import { TAP, TAP_INLINE, TOUCH_TEXT } from "../components/fieldStyles";
import { SM_QUERY, useMediaQuery } from "../hooks/useMediaQuery";
import { SessionSheet } from "./SessionSheet";

/** A session's spend is often a fraction of a cent. */

const DOT: Record<ConnectionStatus, string> = {
  connecting: "bg-console-muted animate-pulse",
  live: "bg-state-running",
  reconnecting: "bg-state-parked animate-pulse",
  // Not pulsing: nothing is being attempted (`SPEC.md`, "Session state").
  offline: "bg-state-failed",
};

export interface SessionHeaderProps {
  session: Session;
  status: ConnectionStatus;
  /** The socket's stop, shared with the composer's own button. */
  onStop: () => void;
}

export function SessionHeader({ session, status, onStop }: SessionHeaderProps) {
  // Below `sm` the strip folds to one row: nothing else in the header is
  // watched while a run streams, and every line of it is a line the transcript
  // does not get on a phone.
  const wide = useMediaQuery(SM_QUERY);
  // The git section is the operator's last step on a session, not something
  // watched while it runs, so it stays folded away until it is asked for — and
  // only then is the project read. Below `sm` it is a sheet over the session
  // box rather than a band that grows the header.
  const [branchOpen, setBranchOpen] = useState(false);
  const [detailsOpen, setDetailsOpen] = useState(false);
  // A crossing of `sm` closes the branch section, as a crossing of `lg` closes
  // the side panel's sheet: nothing opened in one chrome shows up in the other.
  const [seenWide, setSeenWide] = useState(wide);
  if (seenWide !== wide) {
    setSeenWide(wide);
    setBranchOpen(false);
  }
  const detailsId = useId();
  const branchButton = useRef<HTMLButtonElement>(null);
  const detailsButton = useRef<HTMLButtonElement>(null);

  const openBranch = (): void => {
    // One sheet at a time over the session box.
    if (!wide) setPanelSheet(session.id, false);
    setBranchOpen(true);
  };

  /** Closes the branch sheet and hands the focus back to what opened it. */
  const closeBranchSheet = (): void => {
    setBranchOpen(false);
    (detailsOpen ? branchButton.current : detailsButton.current)?.focus();
  };

  const branchToggle = (
    <button
      ref={branchButton}
      type="button"
      aria-expanded={branchOpen}
      onClick={() => {
        if (branchOpen) {
          setBranchOpen(false);
        } else {
          openBranch();
        }
      }}
      className={`font-mono text-xs ${TAP_INLINE} ${
        branchOpen
          ? "text-console-accent"
          : "text-console-muted hover:text-console-text"
      }`}
    >
      branch
    </button>
  );

  const links = (
    <div className="flex flex-wrap items-center gap-2">
      {branchToggle}
      <Link
        to={`/projects/${session.project_id}`}
        className={`text-console-muted hover:text-console-accent font-mono text-xs ${TAP_INLINE}`}
      >
        project
      </Link>
      <CopyLinkButton path={`/sessions/${session.id}`} label="Session link" />
      <PanelsButton sessionId={session.id} />
    </div>
  );

  const actions = (
    <SessionActions
      session={session}
      onStop={onStop}
      onShowBranch={openBranch}
    />
  );

  const failure = session.state === "failed" && session.error !== null && (
    <div className="mt-2">
      <Alert kind="error">{session.error}</Alert>
    </div>
  );

  const waiting = session.state === "parked" && (
    <span className="text-state-parked font-mono text-xs">waiting</span>
  );

  const dot = (
    <span
      role="status"
      aria-label={`Connection ${status}`}
      title={status}
      className={`size-2 shrink-0 rounded-full ${DOT[status]}`}
    />
  );

  const kind = (
    <span className="text-console-muted font-mono text-xs">{session.kind}</span>
  );

  if (!wide) {
    return (
      // At most half the session box, scrolling inside itself past that, so a
      // long failure or an unfolded Details never takes the transcript.
      <header className="border-console-border bg-console-surface max-h-[50%] overflow-y-auto overscroll-contain border-b px-4 py-2">
        <div className="flex min-w-0 items-center gap-2">
          <SessionStatePill state={session.state} error={session.error} />
          {waiting}
          {dot}
          <SessionTitle session={session} />
          <button
            ref={detailsButton}
            type="button"
            aria-expanded={detailsOpen}
            aria-controls={detailsId}
            onClick={() => {
              setDetailsOpen((open) => !open);
            }}
            className={`ml-auto flex shrink-0 items-center gap-1 font-mono text-xs ${TAP_INLINE} ${
              detailsOpen
                ? "text-console-accent"
                : "text-console-muted hover:text-console-text"
            }`}
          >
            {detailsOpen ? (
              <Icon.collapse aria-hidden="true" className={ICON_CLASS} />
            ) : (
              <Icon.expand aria-hidden="true" className={ICON_CLASS} />
            )}
            Details
          </button>
        </div>

        {/* Hidden rather than unmounted, so an open confirmation or an
            action's refusal is still there when Details is opened again. */}
        <div
          id={detailsId}
          hidden={!detailsOpen}
          className={`mt-1 flex-col gap-1 ${detailsOpen ? "flex" : "hidden"}`}
        >
          {/* The kind and the tag lead the metadata's own lines rather than
              taking one of their own: every line here is the transcript's. */}
          <div className="flex flex-wrap items-center gap-x-4 gap-y-1">
            {kind}
            <LaunchSourceTag source={session.launch_source} />
            <Metadata session={session} full />
          </div>
          {links}
          {actions}
        </div>

        {failure}

        {/* Positioned against the session box, not this header: the sheet
            covers the lower part of the box, as the side panel's does. */}
        {branchOpen && (
          <SessionSheet label="Session branch" onClose={closeBranchSheet}>
            <BranchSheetBody session={session} onClose={closeBranchSheet} />
          </SessionSheet>
        )}
      </header>
    );
  }

  return (
    <header className="border-console-border bg-console-surface border-b px-4 py-2">
      <div className="flex flex-wrap items-start justify-between gap-x-4 gap-y-2">
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <div className="flex min-w-0 flex-wrap items-center gap-2">
            <SessionStatePill state={session.state} error={session.error} />
            {waiting}
            {kind}
            {/* Nothing here for a session a person launched: that is what a
                session on this page normally is (`SPEC.md`, "Sessions"). */}
            <LaunchSourceTag source={session.launch_source} />
            {dot}
            <SessionTitle session={session} />
          </div>

          <Metadata session={session} />
        </div>

        <div className="flex shrink-0 flex-col items-end gap-2">
          {links}
          {actions}
        </div>
      </div>

      {failure}

      {branchOpen && (
        <div className="border-console-border mt-2 border-t pt-3">
          <BranchSection session={session} />
        </div>
      )}
    </header>
  );
}

/**
 * The branch sheet's own strip and its scrolling body. It takes the focus when
 * it opens, onto its close button: a dialog is where the focus goes.
 */
function BranchSheetBody({
  session,
  onClose,
}: {
  session: Session;
  onClose: () => void;
}) {
  const close = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    close.current?.focus();
  }, []);

  return (
    <>
      <div className="border-console-border flex items-center gap-1 border-b px-3">
        <span className="text-console-text flex min-w-0 flex-1 items-center gap-1 font-mono text-xs">
          <Icon.branch aria-hidden="true" className={ICON_CLASS} />
          branch
        </span>
        <button
          ref={close}
          type="button"
          aria-label="Close branch"
          onClick={onClose}
          className={`text-console-muted hover:text-console-text p-1 ${TAP}`}
        >
          <Icon.close aria-hidden="true" className={ICON_CLASS} />
        </button>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain p-3">
        <BranchSection session={session} />
      </div>
    </>
  );
}

/**
 * The side panel's opener below `lg`, where the panel is a sheet over the
 * transcript rather than a column with a rail of its own (`SidePanel`). At
 * `lg` the rail is the opener and this button is not shown.
 */
function PanelsButton({ sessionId }: { sessionId: string }) {
  const open = usePanelSheet(sessionId);
  return (
    <button
      type="button"
      id={panelOpenerId(sessionId)}
      aria-expanded={open}
      onClick={() => {
        setPanelSheet(sessionId, !open);
      }}
      className={`flex items-center gap-1 font-mono text-xs lg:hidden ${TAP_INLINE} ${
        open
          ? "text-console-accent"
          : "text-console-muted hover:text-console-text"
      }`}
    >
      <Icon.panelOpen aria-hidden="true" className={ICON_CLASS} />
      Panels
    </button>
  );
}

/**
 * The session's own row of `GitActionsPanel`: under the header band from `sm`
 * up, in a sheet over the session box below it. The panel
 * takes a `Project`, which the session page does not hold, so it is read here
 * and nothing is rendered until it is there.
 */
function BranchSection({ session }: { session: Session }) {
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const project = useQuery(projectQueries.detail(session.project_id));

  const sync = useSyncSession(session.id, session.project_id, {
    onSuccess: (result) => {
      setError(null);
      setNotice(`Synced ${result.ref} at ${shortSha(result.commit)}`);
    },
    onError: (caught: unknown) => {
      setNotice(null);
      logUnexpected(caught);
      setError(errorMessage(caught, "Could not sync the session branch"));
    },
  });

  const workTreeNote = useSessionStore(session.id, selectWorkTreeNote);

  return (
    <div>
      {project.isPending && (
        <p className="text-console-muted font-mono text-xs">
          Loading the project…
        </p>
      )}
      {project.isError && (
        <Alert kind="error">Could not load the project of this session.</Alert>
      )}
      {project.data !== undefined && (
        <GitActionsPanel
          project={project.data}
          sessionId={session.id}
          onSync={() => {
            sync.mutate();
          }}
          syncing={sync.isPending}
          workTreeNote={workTreeNote}
        />
      )}

      {notice !== null && (
        <div className="mt-2">
          <Alert
            kind="success"
            onDismiss={() => {
              setNotice(null);
            }}
          >
            {notice}
          </Alert>
        </div>
      )}
      {error !== null && (
        <div className="mt-2">
          <Alert
            kind="error"
            onDismiss={() => {
              setError(null);
            }}
          >
            {error}
          </Alert>
        </div>
      )}
    </div>
  );
}

/** How far back a rebase outcome is still worth reporting in the form. */
const NOTE_SCAN = 50;

/** `ARCHITECTURE.md`, "Git model": the `work_tree` of the latest `git` event. */
const RECONCILE_NOTE =
  "The last rebase could not update this session's checkout: its work tree is dirty, so the agent has to reconcile it.";

/**
 * The reconciliation note of the most recent rebase outcome in the transcript,
 * or `null` when the last one settled cleanly (`SPEC.md`, "AgentEvent", `git`).
 */
function selectWorkTreeNote(state: SessionStore): string | null {
  const ids = state.order.slice(-NOTE_SCAN);
  for (let index = ids.length - 1; index >= 0; index -= 1) {
    const id = ids[index];
    if (id === undefined) {
      continue;
    }
    const message = state.messages[id];
    if (message === undefined || message.kind !== "system") {
      continue;
    }
    if (message.workTree === undefined) {
      continue;
    }
    return message.workTree === "reconciliation_required"
      ? RECONCILE_NOTE
      : null;
  }
  return null;
}

/**
 * The session's identifiers and counters. `full` is the phone's Details, where
 * there is room for a line each and no hover to show a tooltip: the ids are
 * written out whole there, and every long value breaks inside the box rather
 * than being clipped by it.
 */
function Metadata({
  session,
  full = false,
}: {
  session: Session;
  full?: boolean;
}) {
  const id = (value: string | null): string =>
    full ? (value ?? PLACEHOLDER) : shortId(value);
  return (
    // In Details the list's fields flow in the line its caller starts.
    <dl
      className={`text-console-muted font-mono text-xs ${full ? "contents" : "flex flex-wrap items-center gap-x-4 gap-y-1"}`}
    >
      <Field label="branch" value={session.branch ?? PLACEHOLDER} />
      <Field label="base" value={session.base_ref} />
      <Field
        label="container"
        value={id(session.container_id)}
        title={full ? null : session.container_id}
      />
      <Field
        label="cli"
        value={id(session.cli_session_id)}
        title={full ? null : session.cli_session_id}
      />
      <Field label="cost" value={formatUsd(session.cost_usd, COST_DECIMALS)} />
      <Field label="in" value={formatTokens(session.input_tokens)} />
      <Field label="out" value={formatTokens(session.output_tokens)} />
      <Field label="active" value={formatRelative(session.last_activity_at)} />
    </dl>
  );
}

interface FieldProps {
  label: string;
  value: string;
  /** The unabbreviated value, when the shown one is shortened. */
  title?: string | null;
}

function Field({ label, value, title }: FieldProps) {
  return (
    <div className="flex min-w-0 items-baseline gap-1">
      <dt className="opacity-70">{label}</dt>
      <dd
        className="text-console-text min-w-0 break-all"
        title={title ?? undefined}
      >
        {value}
      </dd>
    </div>
  );
}

function SessionTitle({ session }: { session: Session }) {
  const queryClient = useQueryClient();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(session.title ?? "");
  const [error, setError] = useState<string | null>(null);

  const rename = useMutation({
    mutationFn: (title: string) => updateSession(session.id, { title }),
    onSuccess: (next) => {
      setEditing(false);
      setError(null);
      getSessionStore(session.id).getState().setSession(next);
      queryClient.setQueryData(queryKeys.sessions.detail(session.id), next);
      void queryClient.invalidateQueries({
        queryKey: queryKeys.projects.sessions(session.project_id),
      });
    },
    onError: (caught: unknown) => {
      // The server keeps the old title; so does the header, with the reason.
      setEditing(false);
      logUnexpected(caught);
      setError(errorMessage(caught, "Could not rename the session"));
    },
  });

  const commit = (): void => {
    const trimmed = draft.trim();
    if (trimmed === (session.title ?? "")) {
      setEditing(false);
      return;
    }
    rename.mutate(trimmed);
  };

  const cancelButton = useRef<HTMLButtonElement>(null);
  const cancel = (): void => {
    setDraft(session.title ?? "");
    setEditing(false);
  };

  if (!editing) {
    return (
      <span className="flex min-w-0 items-center gap-2">
        <button
          type="button"
          aria-label="Edit title"
          onClick={() => {
            setDraft(session.title ?? "");
            setError(null);
            setEditing(true);
          }}
          className="text-console-text hover:text-console-accent min-w-0 truncate text-sm font-semibold"
        >
          {session.title ?? (
            <span className="text-console-muted font-normal">untitled</span>
          )}
        </button>
        {error !== null && (
          <span className="text-state-failed text-xs">{error}</span>
        )}
      </span>
    );
  }

  return (
    <span className="flex min-w-0 items-center gap-1">
      <input
        autoFocus
        aria-label="Session title"
        value={draft}
        disabled={rename.isPending}
        onChange={(event) => {
          setDraft(event.target.value);
        }}
        onBlur={(event) => {
          // Tabbing onto the cancel button is not a commit: it is on its way
          // to being a cancel.
          if (event.relatedTarget === cancelButton.current) return;
          commit();
        }}
        onKeyDown={(event) => {
          if (event.key === "Enter") {
            event.preventDefault();
            commit();
          }
          if (event.key === "Escape") {
            event.preventDefault();
            cancel();
          }
        }}
        className={`bg-console-bg border-console-border text-console-text w-64 max-w-full min-w-0 rounded border px-2 py-0.5 text-sm ${TOUCH_TEXT}`}
      />
      {/* A phone has no Escape, so the cancel is a button at every width. Its
          press keeps the focus in the field, or the blur would commit the draft
          before the click could discard it. */}
      <button
        ref={cancelButton}
        type="button"
        aria-label="Cancel rename"
        disabled={rename.isPending}
        onMouseDown={(event) => {
          event.preventDefault();
        }}
        onClick={cancel}
        className={`text-console-muted hover:text-console-text shrink-0 p-0.5 ${TAP_INLINE}`}
      >
        <Icon.close aria-hidden="true" className={ICON_CLASS} />
      </button>
    </span>
  );
}
