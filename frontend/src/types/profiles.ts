// Mirrors `SPEC.md`, "Agent profiles (`/api/projects/{pid}/profiles`)";
// `ProfileKind` is the `profile_kind` enum of `docs/data-model.md`, "Enums".

export type ProfileKind = "conversational" | "ephemeral";

/**
 * The MCP tools a profile can be granted (`SPEC.md`, "MCP tool contracts":
 * the four git tools are profile-gated). The task tools are always allowed and
 * are therefore not listed here — `mcp_tools` is the git allow-list and
 * nothing else. A tool name outside this set is a 400 from the API.
 */
export const PROFILE_GATED_TOOLS = [
  "list_session_branches",
  "merge",
  "rebase",
  "push",
] as const;

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
  /**
   * The dispatcher may start a session of this profile by itself. Only an
   * `ephemeral` profile may carry it, and only while the backend's agent
   * credential resolves without a user (`SPEC.md`, "Agent profiles";
   * `ARCHITECTURE.md`, "Task tracker" → "Unattended launches").
   */
  auto_launch: boolean;
  /**
   * How many live sessions of this profile an unattended launch may leave
   * behind; at least 1, and read whether or not `auto_launch` is set.
   */
  max_concurrent: number;
  created_at: string;
  updated_at: string;
}

/**
 * One role template of `SPEC.md`, "Role profile templates", as
 * `GET /profile-templates` serves it: the same four for every caller, with no
 * project and no ids. Deliberately not a whole `Profile` — a template says
 * what makes the role a role, and every other field of a profile created from
 * one is the documented default of "Agent profiles".
 *
 * `is_default` is informational — it says which template project creation
 * makes the project's default — and the editor never sends it on.
 */
export interface ProfileTemplate {
  name: string;
  kind: ProfileKind;
  backend: string;
  /** Names of the seeded queue states; a project may have renamed them away. */
  serves_states: string[];
  mcp_tools: string[];
  system_prompt: string;
  is_default: boolean;
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
  /** Defaults to `false` when omitted, on `PUT` as well as on `POST`. */
  auto_launch?: boolean;
  /** Defaults to 1 when omitted, on `PUT` as well as on `POST`. */
  max_concurrent?: number;
}
