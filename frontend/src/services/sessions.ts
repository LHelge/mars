// `SPEC.md`, "Sessions": the cross-project list the dashboard reads. The
// project and session views extend this module with the rest of the endpoints.

import type { Session, SessionState } from "../types";
import { apiGet } from "./apiClient";

export interface ListSessionsParams {
  /** One of the five lifecycle states; anything else is a 400. */
  state?: SessionState;
}

/** `GET /sessions` across every project the caller can see. */
export function listSessions(params: ListSessionsParams = {}): Promise<Session[]> {
  const query = new URLSearchParams();
  if (params.state !== undefined) {
    query.set("state", params.state);
  }
  const search = query.toString();
  return apiGet<Session[]>(`/sessions${search === "" ? "" : `?${search}`}`);
}
