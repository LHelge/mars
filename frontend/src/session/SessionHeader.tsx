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
// Escape restores.

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Link } from "react-router";

import { Alert, CopyLinkButton, SessionStatePill } from "../components";
import { ApiError } from "../services/apiClient";
import { queryKeys } from "../services/queryKeys";
import { updateSession } from "../services/sessions";
import type { Session } from "../types";
import {
  formatRelative,
  formatTokens,
  formatUsd,
  PLACEHOLDER,
  shortId,
} from "../utils/format";
import { SessionActions } from "./SessionActions";
import type { ConnectionStatus } from "./sessionStore";
import { getSessionStore } from "./sessionStore";

/** A session's spend is often a fraction of a cent. */
const COST_DECIMALS = 4;

const DOT: Record<ConnectionStatus, string> = {
  connecting: "bg-console-muted animate-pulse",
  live: "bg-state-running",
  reconnecting: "bg-state-parked animate-pulse",
};

export interface SessionHeaderProps {
  session: Session;
  status: ConnectionStatus;
  /** The socket's stop, shared with the composer's own button. */
  onStop: () => void;
}

export function SessionHeader({ session, status, onStop }: SessionHeaderProps) {
  return (
    <header className="border-console-border bg-console-surface border-b px-4 py-2">
      <div className="flex flex-wrap items-start justify-between gap-x-4 gap-y-2">
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <div className="flex min-w-0 flex-wrap items-center gap-2">
            <SessionStatePill state={session.state} error={session.error} />
            {session.state === "parked" && (
              <span className="text-state-parked font-mono text-xs">
                waiting
              </span>
            )}
            <span className="text-console-muted font-mono text-xs">
              {session.kind}
            </span>
            <span
              role="status"
              aria-label={`Connection ${status}`}
              title={status}
              className={`size-2 rounded-full ${DOT[status]}`}
            />
            <SessionTitle session={session} />
          </div>

          <Metadata session={session} />
        </div>

        <div className="flex shrink-0 flex-col items-end gap-2">
          <div className="flex items-center gap-2">
            <Link
              to={`/projects/${session.project_id}`}
              className="text-console-muted hover:text-console-accent font-mono text-xs"
            >
              project
            </Link>
            <CopyLinkButton path={`/sessions/${session.id}`} />
          </div>
          <SessionActions session={session} onStop={onStop} />
        </div>
      </div>

      {session.state === "failed" && session.error !== null && (
        <div className="mt-2">
          <Alert kind="error">{session.error}</Alert>
        </div>
      )}
    </header>
  );
}

function Metadata({ session }: { session: Session }) {
  return (
    <dl className="text-console-muted flex flex-wrap items-center gap-x-4 gap-y-1 font-mono text-xs">
      <Field label="branch" value={session.branch ?? PLACEHOLDER} />
      <Field label="base" value={session.base_ref} />
      <Field
        label="container"
        value={shortId(session.container_id)}
        title={session.container_id}
      />
      <Field
        label="cli"
        value={shortId(session.cli_session_id)}
        title={session.cli_session_id}
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
    <div className="flex items-baseline gap-1">
      <dt className="opacity-70">{label}</dt>
      <dd className="text-console-text" title={title ?? undefined}>
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
      setError(
        caught instanceof ApiError ? caught.error : "Could not rename the session",
      );
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
    <input
      autoFocus
      aria-label="Session title"
      value={draft}
      disabled={rename.isPending}
      onChange={(event) => {
        setDraft(event.target.value);
      }}
      onBlur={commit}
      onKeyDown={(event) => {
        if (event.key === "Enter") {
          event.preventDefault();
          commit();
        }
        if (event.key === "Escape") {
          event.preventDefault();
          setDraft(session.title ?? "");
          setEditing(false);
        }
      }}
      className="bg-console-bg border-console-border text-console-text w-64 max-w-full rounded border px-2 py-0.5 text-sm"
    />
  );
}
