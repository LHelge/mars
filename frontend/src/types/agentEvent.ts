// Mirrors `SPEC.md`, "AgentEvent": the single schema the frontend sees for
// every backend. Written as a discriminated union on `kind` so the session
// store can `switch` exhaustively. Payload fields whose names start with `_`
// are internal and never reach the browser, so they are not modelled here.
//
// There is no `prompt` kind: the pinned CLI never asks the host a question
// under `--permission-mode bypassPermissions --permission-prompts none`
// (ADR 0033; `docs/decisions/0033-no-interactive-prompts-in-v1.md`).

import type { SessionState } from "./sessions";

export interface AgentEventBase {
  /** Per-session, monotonic, starts at 1. */
  seq: number;
  /** RFC 3339, orchestrator observation time. */
  ts: string;
  /** Present on everything emitted inside a subagent. */
  parent_tool_use_id?: string;
  /** Backend message id when the backend provides one. */
  message_id?: string;
}

export interface McpServerStatus {
  name: string;
  status: string;
}

/** The `detail` of a `git` event, one shape per `op` (`SPEC.md`, "AgentEvent"). */
export type GitDetail =
  | { ref: string; commit?: string; error?: string }
  | {
      source: string;
      target: string;
      commit?: string;
      fast_forward?: boolean;
      conflicts?: string[];
      requested_by: string;
      error?: string;
    }
  | {
      branch: string;
      onto: string;
      commit?: string;
      conflicts?: string[];
      work_tree?: "updated" | "reconciliation_required" | "not_applicable";
      requested_by: string;
      error?: string;
    }
  | {
      ref: string;
      remote_branch: string;
      commit?: string;
      force: boolean;
      compare_url?: string;
      requested_by: string;
      error?: string;
    };

export type GitOp = "sync" | "merge" | "rebase" | "push";

export type AgentEvent = AgentEventBase &
  (
    | {
        kind: "init";
        cli_session_id: string;
        model?: string;
        tools: string[];
        mcp_servers: McpServerStatus[];
        resumed: boolean;
      }
    | {
        kind: "user_message";
        text: string;
        user_id: string | null;
        client_id?: string;
      }
    /** Partial messages only. */
    | { kind: "text_delta"; text: string }
    /** A complete assistant text block. */
    | { kind: "text"; text: string }
    | { kind: "thinking"; text: string; redacted: boolean }
    | { kind: "tool_call"; tool_use_id: string; name: string; input: unknown }
    | {
        kind: "tool_result";
        tool_use_id: string;
        /** `SPEC.md` writes `string | unknown`, which is `unknown`: a renderer narrows it. */
        content: unknown;
        is_error: boolean;
        truncated: boolean;
      }
    | {
        kind: "permission_denied";
        tool_use_id?: string;
        name: string;
        reason: string;
      }
    | {
        kind: "subagent_start";
        tool_use_id: string;
        description: string;
        agent_type?: string;
      }
    | { kind: "subagent_end"; tool_use_id: string; is_error: boolean }
    | {
        kind: "result";
        subtype: string;
        /** `completed`, or `aborted_streaming` for a turn a stop interrupted. */
        terminal_reason?: string;
        is_error: boolean;
        num_turns: number;
        duration_ms: number;
        cost_usd?: number;
        usage?: unknown;
        permission_denials: unknown[];
      }
    | { kind: "error"; message: string; fatal: boolean }
    | {
        kind: "state_change";
        from: SessionState;
        to: SessionState;
        reason: string;
        signal?: "SIGINT" | "SIGTERM";
      }
    /** For example an undeclared secret. */
    | { kind: "launch_warning"; message: string }
    | { kind: "git"; op: GitOp; ok: boolean; detail: GitDetail }
    /** An untranslated native line. */
    | { kind: "raw"; backend: "claude"; native: unknown }
  );

export type AgentEventKind = AgentEvent["kind"];
