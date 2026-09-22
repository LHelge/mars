// Mirrors `SPEC.md`, "Agent profiles (`/api/projects/{pid}/profiles`)";
// `ProfileKind` is the `profile_kind` enum of `docs/data-model.md`, "Enums".

import type { AgentBackend } from "./secrets";

/** `docs/data-model.md`, "Enums", `profile_kind`. */
export const PROFILE_KINDS = ["conversational", "ephemeral"] as const;

export type ProfileKind = (typeof PROFILE_KINDS)[number];

/** The kind a select's string is, or `undefined` for anything else. */
export function parseProfileKind(value: string): ProfileKind | undefined {
  return PROFILE_KINDS.find((kind) => kind === value);
}

/** `SPEC.md`, "Agent profiles": `permission_mode` must be `bypass`. */
export const PERMISSION_MODES = ["bypass"] as const;

export type PermissionMode = (typeof PERMISSION_MODES)[number];

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

/** One entry of `mcp_tools`; a misspelled one no longer type-checks. */
export type ProfileGatedTool = (typeof PROFILE_GATED_TOOLS)[number];

export interface Profile {
  id: string;
  project_id: string;
  name: string;
  kind: ProfileKind;
  backend: AgentBackend;
  model: string | null;
  system_prompt: string | null;
  permission_mode: PermissionMode;
  image: string;
  runtime: string | null;
  mcp_tools: ProfileGatedTool[];
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
  /**
   * The UTC 5-field cron expression a session of this profile is launched on,
   * or `null` for a profile nothing schedules. Only an `ephemeral` profile may
   * carry one, and only while the backend's agent credential resolves without
   * a user (`SPEC.md`, "Agent profiles" → "Scheduled profiles"; ADR 0043).
   */
  schedule_cron: string | null;
  /** The `message` a scheduled run is given; set exactly when `schedule_cron` is. */
  schedule_prompt: string | null;
  /**
   * When the scheduler last decided a tick of this profile fires. Read-only —
   * the job is its only writer — and cleared when the schedule is.
   */
  last_scheduled_at: string | null;
  /**
   * The first occurrence of `schedule_cron` strictly after now, in UTC,
   * computed by the server on every read so no client needs a cron parser.
   * `null` without a schedule, and for an expression that can never fire.
   */
  next_scheduled_at: string | null;
  created_at: string;
  updated_at: string;
}

/**
 * One role template of `SPEC.md`, "Role profile templates", as
 * `GET /profile-templates` serves it: the same list for every caller, with no
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
  backend: AgentBackend;
  /** Names of the seeded queue states; a project may have renamed them away. */
  serves_states: string[];
  mcp_tools: ProfileGatedTool[];
  system_prompt: string;
  is_default: boolean;
  /**
   * The cron expression a scheduled template runs on, or `null` on a template
   * a person launches. Set exactly with `schedule_prompt`, and only on a
   * template whose `kind` is `ephemeral` (`SPEC.md`, "Agent profiles" →
   * "Scheduled profiles").
   */
  schedule_cron: string | null;
  /** The `message` each scheduled run is given; `null` exactly when `schedule_cron` is. */
  schedule_prompt: string | null;
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
  backend?: AgentBackend;
  model?: string | null;
  system_prompt?: string | null;
  permission_mode?: PermissionMode;
  image?: string;
  runtime?: string | null;
  mcp_tools?: ProfileGatedTool[];
  secrets?: string[];
  serves_states?: string[];
  partial_messages?: boolean;
  idle_timeout_secs?: number;
  /** Defaults to `false` when omitted, on `PUT` as well as on `POST`. */
  auto_launch?: boolean;
  /** Defaults to 1 when omitted, on `PUT` as well as on `POST`. */
  max_concurrent?: number;
  /**
   * The schedule. The two fields stand or fall together: an expression without
   * a non-blank prompt is a 400, a prompt without an expression is a 400, and
   * both absent or blank is the profile having no schedule. `last_scheduled_at`
   * and `next_scheduled_at` are the scheduler's and are never sent.
   */
  schedule_cron?: string | null;
  schedule_prompt?: string | null;
}
