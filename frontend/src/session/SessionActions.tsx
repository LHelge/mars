// What an operator can do to a running session, gated by its state
// (`SPEC.md`, "Sessions"; `ARCHITECTURE.md`, "Session lifecycle").
//
//   Stop    running                 SIGINT now, SIGTERM after the grace period
//   End     creating/running/parked stop, fetch back, close as `done`
//   Sync    running/parked/done     fetch the session branch into the mirror
//   Retry   failed, conversational  relaunch, optionally with a new message
//   Delete  done/failed             remove the session and leave the page
//
// Ephemeral sessions run one prompt and end; they are never retried, so the
// button is not there to be refused. Destructive actions confirm in place —
// the second press is the confirmation — rather than in a modal, because the
// header is already the place the operator is looking.
//
// Every refusal is the server's own sentence: `SPEC.md` phrases why a sync on a
// `creating` session or a delete of a running one cannot happen better than the
// client could guess.

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { useNavigate } from "react-router";

import { Alert, SubmitButton } from "../components";
import { ApiError } from "../services/apiClient";
import { queryKeys } from "../services/queryKeys";
import {
  deleteSession,
  endSession,
  retrySession,
  syncSession,
} from "../services/sessions";
import type { Session } from "../types";
import { shortSha } from "../utils/format";
import { getSessionStore } from "./sessionStore";

export interface SessionActionsProps {
  session: Session;
  /** The socket's stop, which falls back to `POST /sessions/{id}/stop`. */
  onStop: () => void;
}

function message(caught: unknown, fallback: string): string {
  if (caught instanceof ApiError) {
    return caught.error;
  }
  console.error(caught);
  return fallback;
}

export function SessionActions({ session, onStop }: SessionActionsProps) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();

  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  /** `end` or `delete` once pressed; the next press carries it out. */
  const [confirming, setConfirming] = useState<"end" | "delete" | null>(null);
  const [retryOpen, setRetryOpen] = useState(false);
  const [retryMessage, setRetryMessage] = useState("");

  const id = session.id;
  const state = session.state;

  /** The authoritative session a lifecycle call returns, into both readers. */
  const adopt = (next: Session): void => {
    getSessionStore(id).getState().setSession(next);
    queryClient.setQueryData(queryKeys.sessions.detail(id), next);
    void queryClient.invalidateQueries({
      queryKey: queryKeys.projects.sessions(next.project_id),
    });
  };

  const begin = (): void => {
    setError(null);
    setNotice(null);
  };

  const end = useMutation({
    mutationFn: () => endSession(id),
    onSuccess: (next) => {
      setConfirming(null);
      adopt(next);
    },
    onError: (caught: unknown) => {
      setConfirming(null);
      setError(message(caught, "Could not end the session"));
    },
  });

  const sync = useMutation({
    mutationFn: () => syncSession(id),
    onSuccess: (result) => {
      setNotice(`Synced ${result.ref} at ${shortSha(result.commit)}`);
    },
    onError: (caught: unknown) => {
      setError(message(caught, "Could not sync the session branch"));
    },
  });

  const retry = useMutation({
    mutationFn: (text: string) =>
      retrySession(id, text === "" ? {} : { message: text }),
    onSuccess: (next) => {
      setRetryOpen(false);
      setRetryMessage("");
      adopt(next);
    },
    onError: (caught: unknown) => {
      setError(message(caught, "Could not retry the session"));
    },
  });

  const remove = useMutation({
    mutationFn: () => deleteSession(id),
    onSuccess: () => {
      void queryClient.invalidateQueries({
        queryKey: queryKeys.projects.sessions(session.project_id),
      });
      // Leaving unmounts the socket; the session is gone, so nothing reopens it.
      void navigate(`/projects/${session.project_id}?tab=sessions`);
    },
    onError: (caught: unknown) => {
      setConfirming(null);
      setError(message(caught, "Could not delete the session"));
    },
  });

  const canEnd = state === "creating" || state === "running" || state === "parked";
  const canSync = state === "running" || state === "parked" || state === "done";
  const canRetry = state === "failed" && session.kind === "conversational";
  const canDelete = state === "done" || state === "failed";

  return (
    <div className="flex min-w-0 flex-col items-end gap-2">
      <div className="flex flex-wrap items-center justify-end gap-2">
        {state === "running" && (
          <SubmitButton
            type="button"
            variant="danger"
            loading={false}
            onClick={() => {
              begin();
              onStop();
            }}
          >
            Stop
          </SubmitButton>
        )}

        {canEnd && (
          <SubmitButton
            type="button"
            variant="ghost"
            loading={end.isPending}
            onClick={() => {
              begin();
              if (confirming === "end") {
                end.mutate();
              } else {
                setConfirming("end");
              }
            }}
          >
            {confirming === "end" ? "Confirm end" : "End"}
          </SubmitButton>
        )}

        {canSync && (
          <SubmitButton
            type="button"
            variant="ghost"
            loading={sync.isPending}
            onClick={() => {
              begin();
              sync.mutate();
            }}
          >
            Sync
          </SubmitButton>
        )}

        {canRetry && (
          <SubmitButton
            type="button"
            variant="ghost"
            loading={false}
            onClick={() => {
              begin();
              setRetryOpen((open) => !open);
            }}
          >
            Retry
          </SubmitButton>
        )}

        {canDelete && (
          <SubmitButton
            type="button"
            variant="danger"
            loading={remove.isPending}
            onClick={() => {
              begin();
              if (confirming === "delete") {
                remove.mutate();
              } else {
                setConfirming("delete");
              }
            }}
          >
            {confirming === "delete" ? "Confirm delete" : "Delete"}
          </SubmitButton>
        )}
      </div>

      {retryOpen && canRetry && (
        <form
          className="flex w-full max-w-md items-center gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            begin();
            retry.mutate(retryMessage.trim());
          }}
        >
          <input
            aria-label="Retry message"
            value={retryMessage}
            placeholder="Message to relaunch with (optional)"
            onChange={(event) => {
              setRetryMessage(event.target.value);
            }}
            className="bg-console-surface border-console-border text-console-text placeholder:text-console-muted min-w-0 flex-1 rounded border px-2 py-1 font-mono text-xs"
          />
          <SubmitButton loading={retry.isPending}>Relaunch</SubmitButton>
        </form>
      )}

      {notice !== null && (
        <Alert
          kind="success"
          onDismiss={() => {
            setNotice(null);
          }}
        >
          {notice}
        </Alert>
      )}
      {error !== null && (
        <Alert
          kind="error"
          onDismiss={() => {
            setError(null);
          }}
        >
          {error}
        </Alert>
      )}
    </div>
  );
}
