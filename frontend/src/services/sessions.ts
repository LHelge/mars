// `SPEC.md`, "Sessions": the cross-project list the dashboard reads plus every
// project- and session-scoped endpoint the session view uses.

import type {
  EventsPage,
  Session,
  SessionCreateInput,
  SessionInput,
  SessionState,
  SyncResult,
  Task,
} from "../types";
import { apiDelete, apiGet, apiPost, apiPut, seg } from "./apiClient";

/**
 * `GET /sessions` across every project the caller can see, optionally narrowed
 * to one of the five lifecycle states (anything else is a 400).
 *
 * The filter is spelled the same way here and in `listProjectSessions` — one
 * optional `state` argument — because they are the same filter on the same
 * resource and `projectQueries.sessions` keys them alike.
 */
export function listSessions(state?: SessionState): Promise<Session[]> {
  return apiGet<Session[]>("/sessions", { query: { state } });
}

export function listProjectSessions(
  pid: string,
  state?: SessionState,
): Promise<Session[]> {
  return apiGet<Session[]>(`/projects/${seg(pid)}/sessions`, {
    query: { state },
  });
}

export function getSession(id: string): Promise<Session> {
  return apiGet<Session>(`/sessions/${seg(id)}`);
}

/** 201 with the created session, `state: creating`. */
export function createSession(
  pid: string,
  input: SessionCreateInput,
): Promise<Session> {
  return apiPost<Session>(`/projects/${seg(pid)}/sessions`, input);
}

export function updateSession(
  id: string,
  body: { title: string },
): Promise<Session> {
  return apiPut<Session>(`/sessions/${seg(id)}`, body);
}

/** 204; the session must be `done` or `failed`. */
export function deleteSession(id: string): Promise<void> {
  return apiDelete(`/sessions/${seg(id)}`);
}

export interface ListEventsParams {
  /** Exclusive: paging backwards passes the lowest `seq` of the page just received. */
  before?: number;
  /** 1–500, default 100. */
  limit?: number;
}

/**
 * A page of history, newest-last, ending just before `before`. `signal`
 * abandons a page whose reader has gone — a closed session view, or a query
 * TanStack cancelled — rather than parsing up to 500 events into nothing.
 */
export function listEvents(
  id: string,
  params: ListEventsParams = {},
  signal?: AbortSignal,
): Promise<EventsPage> {
  return apiGet<EventsPage>(`/sessions/${seg(id)}/events`, {
    query: { before: params.before, limit: params.limit },
    signal,
  });
}

/**
 * 202 with no body: acceptance by the orchestrator, not delivery (ADR 0020).
 *
 * `clientId` is the same echo key the socket sends: the `user_message` that
 * records this input carries it back, so an input sent here while the socket
 * is closed still reconciles its optimistic message.
 */
export function sendInput(
  id: string,
  input: SessionInput,
  clientId?: string,
): Promise<void> {
  return apiPost<void>(
    `/sessions/${seg(id)}/input`,
    clientId === undefined ? input : { ...input, client_id: clientId },
  );
}

/** 202 with no body: SIGINT, then SIGTERM after the grace period. */
export function stopSession(id: string): Promise<void> {
  return apiPost<void>(`/sessions/${seg(id)}/stop`);
}

/** Stop, fetch back and close: `done`, or `failed` if the run ended that way. */
export function endSession(id: string): Promise<Session> {
  return apiPost<Session>(`/sessions/${seg(id)}/end`);
}

/** Conversational only, from `failed`; relaunches at once when `message` is given. */
export function retrySession(
  id: string,
  body: { message?: string } = {},
): Promise<Session> {
  return apiPost<Session>(`/sessions/${seg(id)}/retry`, body);
}

/** Fetches the session branch into the mirror. */
export function syncSession(id: string): Promise<SyncResult> {
  return apiPost<SyncResult>(`/sessions/${seg(id)}/sync`);
}

export function listSessionTasks(id: string): Promise<Task[]> {
  return apiGet<Task[]>(`/sessions/${seg(id)}/tasks`);
}
