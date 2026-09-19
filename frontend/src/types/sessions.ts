// Mirrors `SPEC.md`, "Sessions (`/api/projects/{pid}/sessions`, `/api/sessions/{id}`)";
// `SessionState` and `SessionKind` are the `session_state` and `profile_kind` enums
// of `docs/data-model.md`, "Enums".

export type SessionState = "creating" | "running" | "parked" | "done" | "failed";

/** The profile's kind at launch, not a separate enum. */
export type SessionKind = "conversational" | "ephemeral";

export interface Session {
  id: string;
  project_id: string;
  profile_id: string;
  kind: SessionKind;
  created_by: string | null;
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
