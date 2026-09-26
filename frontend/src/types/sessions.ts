// Mirrors `SPEC.md`, "Sessions (`/api/projects/{pid}/sessions`, `/api/sessions/{id}`)";
// `SessionState` and `SessionKind` are the `session_state` and `profile_kind` enums
// of `docs/data-model.md`, "Enums".

import type { AgentEvent } from "./agentEvent";

export type SessionState =
  "creating" | "running" | "parked" | "done" | "failed";

/** The profile's kind at launch, not a separate enum. */
export type SessionKind = "conversational" | "ephemeral";

/**
 * Who launched the session: a person over the sessions endpoint, the
 * dispatcher, or a schedule (`SPEC.md`, "Sessions"; `ARCHITECTURE.md`, "Task
 * tracker" → "Unattended launches").
 *
 * It is the only record of that. `created_by` cannot stand in for it: deleting
 * a user nulls that field on the sessions they launched, so a null there means
 * "the user is gone", never "nobody launched it".
 */
export type LaunchSource = "user" | "dispatcher" | "schedule";

export interface Session {
  id: string;
  project_id: string;
  profile_id: string;
  kind: SessionKind;
  created_by: string | null;
  launch_source: LaunchSource;
  title: string | null;
  task_id: string | null;
  handoff_id: string | null;
  state: SessionState;
  base_ref: string;
  branch: string | null;
  container_id: string | null;
  cli_session_id: string | null;
  last_seq: number;
  last_activity_at: string;
  cost_usd: number;
  input_tokens: number;
  output_tokens: number;
  error: string | null;
  created_at: string;
  parked_at: string | null;
  ended_at: string | null;
}

/**
 * `POST /projects/{pid}/sessions`. `task_id` accepts a task's UUID or its
 * per-project number; with it the session claims the task as it is created.
 */
export interface SessionCreateInput {
  profile_id: string;
  /** An integration head, upstream-tracking ref, tag or commit id; defaults per the project or hand-off. */
  base_ref?: string;
  title?: string;
  message?: string;
  task_id?: string | number;
}

/**
 * `SPEC.md`, "WebSocket: session stream": `message` is the only kind. The CLI
 * never waits on stdin for an answer, so there is no `answer` input and no
 * `prompt` event (ADR 0033).
 */
export type SessionInput = { kind: "message"; text: string };

/** `GET /sessions/{id}/events`: a page of history, newest-last. */
export interface EventsPage {
  events: AgentEvent[];
  has_more: boolean;
}

/** `POST /sessions/{id}/sync`: the session ref and the commit it now points at. */
export interface SyncResult {
  ref: string;
  commit: string;
}
