// Mirrors `SPEC.md`, "Agent profiles (`/api/projects/{pid}/profiles`)";
// `ProfileKind` is the `profile_kind` enum of `docs/data-model.md`, "Enums".

export type ProfileKind = "conversational" | "ephemeral";

export interface Profile {
  id: string;
  project_id: string;
  name: string;
  kind: ProfileKind;
  backend: string;
  model: string | null;
  system_prompt: string | null;
  /** Must be `bypass` in v1; typed as a string so a new mode needs no change here. */
  permission_mode: string;
  image: string;
  runtime: string | null;
  mcp_tools: string[];
  secrets: string[];
  /** Names of the project's `queue` task states this profile picks work up from. */
  serves_states: string[];
  partial_messages: boolean;
  idle_timeout_secs: number;
  is_default: boolean;
  created_at: string;
  updated_at: string;
}

/**
 * The body of `POST` and `PUT`: the profile without ids and timestamps. `PUT`
 * replaces the whole profile, so an omitted field takes its default again.
 * Only `name` is required; `partial_messages` is optional so the backend
 * default for the kind applies.
 */
export interface ProfileInput {
  name: string;
  kind?: ProfileKind;
  backend?: string;
  model?: string | null;
  system_prompt?: string | null;
  permission_mode?: string;
  image?: string;
  runtime?: string | null;
  mcp_tools?: string[];
  secrets?: string[];
  serves_states?: string[];
  partial_messages?: boolean;
  idle_timeout_secs?: number;
}
