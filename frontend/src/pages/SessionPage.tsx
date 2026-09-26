// `/sessions/:id` (`SPEC.md`, "Frontend", Routes): the route glue around
// `SessionView`.
//
// The page reads the session once over REST so the view has something to draw
// before the socket says anything, then mounts `useSessionSocket` — exactly
// once, keyed by the id — and lets it own the transcript from there. The
// socket's own `start()` loads the first page of history, so nothing here
// touches events.
//
// Two readers of one session: the store, which the socket keeps current from
// `session` frames, and the TanStack cache, which every other view reads. The
// fresher of the REST row and the store is what is displayed — a retained
// store can be minutes stale on a return visit — and every `session` frame is
// written back into the cache so a list opened afterwards is not stale.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { useParams } from "react-router";

import { Alert } from "../components/Alert";
import { LoadingState } from "../components/LoadingState";
import { PageLayout } from "../components/PageLayout";
import { QueryErrorAlert } from "../components/QueryErrorAlert";
import { ApiError } from "../services/apiClient";
import { errorMessage, isNotFound } from "../services/errorMessage";
import { queryKeys } from "../services/queryKeys";
import { getSession } from "../services/sessions";
import {
  getSessionStore,
  isNewerSession,
  SessionView,
  useSessionSocket,
  useSessionStore,
} from "../session";
import type { Session } from "../types";
import { isUuid } from "../utils/uuid";
import { NotFoundPage } from "./NotFoundPage";

export function SessionPage() {
  const params = useParams();

  // An id that is not a UUID names no session: answer without asking.
  if (!isUuid(params.id)) {
    return <NotFoundPage />;
  }

  // Keyed by the id so switching sessions remounts: a new socket, a new store.
  return <SessionRoute key={params.id} id={params.id} />;
}

function SessionRoute({ id }: { id: string }) {
  const session = useQuery({
    queryKey: queryKeys.sessions.detail(id),
    queryFn: () => getSession(id),
  });

  if (session.isError && isNotFound(session.error)) {
    return <NotFoundPage />;
  }

  const loaded = session.data;

  // Once the session is known the socket owns the view; a failed re-read of
  // the REST row never takes the transcript down with it (`SPEC.md`,
  // "Frontend", Read failures), and what it would have said is already the
  // socket's to report.
  if (loaded !== undefined) {
    return <SessionLive id={id} loaded={loaded} />;
  }

  if (session.isPending) {
    return (
      <PageLayout title="Session">
        <LoadingState label="Loading session" />
      </PageLayout>
    );
  }

  const forbidden =
    session.error instanceof ApiError && session.error.status === 403;

  return (
    <PageLayout title="Session">
      {forbidden ? (
        // A refusal, not a failure: there is nothing to try again.
        <Alert kind="error">You do not have access to this session.</Alert>
      ) : (
        <QueryErrorAlert
          query={session}
          message={errorMessage(session.error, "Could not load the session.")}
        />
      )}
    </PageLayout>
  );
}

/**
 * Split from `SessionRoute` so the socket is mounted only once the session is
 * known to exist: a 404 never opens a connection.
 */
function SessionLive({ id, loaded }: { id: string; loaded: Session }) {
  const queryClient = useQueryClient();
  const socket = useSessionSocket(id);
  const stored = useSessionStore(id, (state) => state.session);
  // A retained store is only ahead of this read while its connection is the
  // one keeping it current. On a return visit it holds the session as the
  // last visit left it — possibly parked or failed since, possibly minutes
  // old — and the socket's first `session` frame may be seconds away or never
  // arrive, so the fresher of the two is what the header shows and what the
  // store is seeded with.
  const session =
    stored === null || isNewerSession(loaded, stored) ? loaded : stored;

  useEffect(() => {
    const store = getSessionStore(id).getState();
    const held = store.session;
    if (held === null || isNewerSession(loaded, held)) store.setSession(loaded);
  }, [id, loaded]);

  // Every `session` frame the socket folds is the freshest copy there is.
  useEffect(() => {
    return getSessionStore(id).subscribe((next, previous) => {
      if (next.session === null || next.session === previous.session) return;
      queryClient.setQueryData(queryKeys.sessions.detail(id), next.session);
    });
  }, [id, queryClient]);

  return (
    <PageLayout>
      <SessionView session={session} socket={socket} />
    </PageLayout>
  );
}
