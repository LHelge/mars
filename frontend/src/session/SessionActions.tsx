// What an operator can do to a running session, gated by its state
// (`SPEC.md`, "Sessions"; `ARCHITECTURE.md`, "Session lifecycle").
//
// Which button each state offers is `sessionActions()` in
// `sessionActionRules.ts`, which is where the state table is written out and
// tested; this file is what the buttons do.
//
// Ephemeral sessions run one prompt and end; they are never retried, so the
// button is not there to be refused. Destructive actions confirm in place, in
// the console's one confirmation panel (`components/ConfirmPanel.tsx`), rather
// than in a modal: the header is already the place the operator is looking.
//
// Every refusal is the server's own sentence: `SPEC.md` phrases why a sync on a
// `creating` session or a delete of a running one cannot happen better than the
// client could guess.
//
// `Stop` is the one action that is also offered elsewhere — the composer has
// the same button — so its behaviour is `useStopSession` and not this file's.

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { useNavigate } from "react-router";

import { Alert } from "../components/Alert";
import { ConfirmPanel } from "../components/ConfirmPanel";
import { Icon } from "../components/icons";
import { SubmitButton } from "../components/SubmitButton";
import { SYNC_TITLE } from "../components/git/syncHint";
import { errorMessage, logUnexpected } from "../services/errorMessage";
import { queryKeys } from "../services/queryKeys";
import { deleteSession, endSession, retrySession } from "../services/sessions";
import type { Session } from "../types";
import { shortSha } from "../utils/format";
import { sessionActions } from "./sessionActionRules";
import { disposeSessionStore, getSessionStore } from "./sessionStore";
import { useStopSession } from "./useStopSession";
import { useSyncSession } from "./useSyncSession";

export interface SessionActionsProps {
  session: Session;
  /** The socket's stop, which falls back to `POST /sessions/{id}/stop`. */
  onStop: () => void;
}

function message(caught: unknown, fallback: string): string {
  logUnexpected(caught);
  return errorMessage(caught, fallback);
}

export function SessionActions({ session, onStop }: SessionActionsProps) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();

  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  /** `end` or `delete` once pressed; the panel below carries it out. */
  const [confirming, setConfirming] = useState<"end" | "delete" | null>(null);
  const [retryOpen, setRetryOpen] = useState(false);
  const [retryMessage, setRetryMessage] = useState("");

  const id = session.id;
  const can = sessionActions(session);
  // The header's `Stop` and the composer's are one button in two places: the
  // same request, the same `Stopping…` while it is outstanding, the same way
  // out if no state change arrives.
  const stop = useStopSession(id, onStop);

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

  // The same mutation the header's Branch section offers (`useSyncSession`).
  const sync = useSyncSession(id, session.project_id, {
    onSuccess: (result) => {
      setNotice(`Synced ${result.ref} at ${shortSha(result.commit)}`);
    },
    onError: (caught) => {
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
      // Nothing will ever resume this transcript, so it is not kept for a
      // return visit the way a closed session's is (`sessionStore`, the
      // registry).
      disposeSessionStore(id);
      // Leaving unmounts the socket; the session is gone, so nothing reopens it.
      void navigate(`/projects/${session.project_id}?tab=sessions`);
    },
    onError: (caught: unknown) => {
      setConfirming(null);
      setError(message(caught, "Could not delete the session"));
    },
  });

  return (
    <div className="flex min-w-0 flex-col items-end gap-2">
      <div className="flex flex-wrap items-center justify-end gap-2">
        {can.stop && (
          <SubmitButton
            type="button"
            variant="danger"
            disabled={stop.stopping}
            icon={Icon.stop}
            onClick={() => {
              begin();
              stop.requestStop();
            }}
          >
            {stop.label}
          </SubmitButton>
        )}

        {can.end && (
          <SubmitButton
            type="button"
            variant="ghost"
            loading={end.isPending}
            icon={Icon.end}
            disabled={confirming === "end"}
            onClick={() => {
              begin();
              setConfirming("end");
            }}
          >
            End
          </SubmitButton>
        )}

        {can.sync && (
          <SubmitButton
            type="button"
            variant="ghost"
            loading={sync.isPending}
            icon={Icon.sync}
            title={SYNC_TITLE}
            onClick={() => {
              begin();
              sync.mutate();
            }}
          >
            Sync
          </SubmitButton>
        )}

        {can.retry && (
          <SubmitButton
            type="button"
            variant="ghost"
            icon={Icon.retry}
            onClick={() => {
              begin();
              setRetryOpen((open) => !open);
            }}
          >
            Retry
          </SubmitButton>
        )}

        {can.delete && (
          <SubmitButton
            type="button"
            variant="danger"
            loading={remove.isPending}
            icon={Icon.delete}
            disabled={confirming === "delete"}
            onClick={() => {
              begin();
              setConfirming("delete");
            }}
          >
            Delete
          </SubmitButton>
        )}
      </div>

      {confirming !== null && (
        <div className="w-full max-w-md">
          <ConfirmPanel
            message={
              confirming === "end"
                ? "End this session? The container stops and the agent cannot be given anything more to do."
                : "Delete this session? Its transcript and events go with it; the branch it left in the git mirror stays."
            }
            confirmLabel={
              confirming === "end" ? "End the session" : "Delete the session"
            }
            pending={confirming === "end" ? end.isPending : remove.isPending}
            onConfirm={() => {
              if (confirming === "end") {
                end.mutate();
              } else {
                remove.mutate();
              }
            }}
            onCancel={() => {
              setConfirming(null);
            }}
          />
        </div>
      )}

      {retryOpen && can.retry && (
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
          <SubmitButton loading={retry.isPending} icon={Icon.launch}>
            Relaunch
          </SubmitButton>
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
