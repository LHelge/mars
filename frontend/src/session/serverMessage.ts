// The session WebSocket's text-frame boundary: the one place a JSON string
// from `/ws/sessions/{id}` becomes a `ServerMessage` the transcript store may
// be handed (`SPEC.md`, "WebSocket: session stream", "AgentEvent").
//
// Two different things can be wrong with a frame, and they get opposite
// answers:
//
// - **Malformed.** Not JSON, not an object, or a field the reducer reads is
//   missing or of the wrong type. Nothing is applied and the cursor does not
//   move: `applyEvent` would otherwise fold half an event and then set
//   `lastSeq` past it, and a transcript with a hole in it is worse than one
//   frame short.
// - **Unknown.** A well-formed message whose `type` this build has never heard
//   of, which is what a tab left open across an orchestrator upgrade sees.
//   Ignored, because a `ServerMessage` carries no cursor: there is nothing to
//   lose by skipping one and nothing this build could do with it.
//
// An unknown *event* `kind` is the third case and is neither of these. It does
// carry a cursor, so it is accepted here and rendered as a `raw` row by
// `foldEvent`'s default arm; dropping it would leave `lastSeq` behind the
// server's and replay the gap on every reconnect.
//
// Diagnostics name the field that failed and never its value: a frame is agent
// output, and the query string it arrived on carries an access token.

import type {
  AgentEvent,
  AgentEventKind,
  ServerMessage,
  Session,
} from "../types";
import {
  isBoolean,
  isNumber,
  isOptional,
  isRecord,
  isSeq,
  isString,
  isStringArray,
  parseJson,
} from "../utils/wire";

export type FrameOutcome =
  | { ok: true; message: ServerMessage }
  /** From a newer orchestrator: ignored, with nothing applied. */
  | { ok: false; unknown: true; detail: string }
  /** Off-contract: rejected, with nothing applied. */
  | { ok: false; unknown: false; detail: string };

function bad(detail: string): FrameOutcome {
  return { ok: false, unknown: false, detail };
}

function mcpServers(value: unknown): boolean {
  return (
    Array.isArray(value) &&
    value.every(
      (entry) =>
        isRecord(entry) && isString(entry.name) && isString(entry.status),
    )
  );
}

/**
 * The per-kind field checks, keyed by `kind`. Each one covers exactly what
 * `foldEvent` reads off that kind (`session/sessionStore.ts`) — a field the
 * fold never touches is the server's business, and checking it here would only
 * reject events this client could have rendered.
 *
 * `input`, `content`, `usage`, `native` and `detail` are `unknown` in the
 * contract itself, so there is nothing to check about them.
 */
type FieldCheck = (event: Record<string, unknown>) => boolean;

const EVENT_FIELDS: Record<AgentEventKind, FieldCheck> = {
  init: (e) =>
    isString(e.cli_session_id) &&
    isOptional(e.model, isString) &&
    isStringArray(e.tools) &&
    mcpServers(e.mcp_servers) &&
    isBoolean(e.resumed),
  user_message: (e) => isString(e.text) && isOptional(e.client_id, isString),
  text_delta: (e) => isString(e.text),
  text: (e) => isString(e.text),
  thinking: (e) => isString(e.text) && isBoolean(e.redacted),
  tool_call: (e) => isString(e.tool_use_id) && isString(e.name),
  tool_result: (e) =>
    isString(e.tool_use_id) && isBoolean(e.is_error) && isBoolean(e.truncated),
  permission_denied: (e) =>
    isString(e.name) &&
    isString(e.reason) &&
    isOptional(e.tool_use_id, isString),
  subagent_start: (e) =>
    isString(e.tool_use_id) &&
    isString(e.description) &&
    isOptional(e.agent_type, isString),
  subagent_end: (e) => isString(e.tool_use_id) && isBoolean(e.is_error),
  result: (e) =>
    isString(e.subtype) &&
    isBoolean(e.is_error) &&
    isNumber(e.num_turns) &&
    isNumber(e.duration_ms) &&
    isOptional(e.cost_usd, isNumber),
  error: (e) => isString(e.message) && isBoolean(e.fatal),
  // `from`, `to` and `signal` are enums the orchestrator only ever adds to, so
  // they are checked as strings: a state this build cannot name still renders
  // as the sentence the fold builds from it.
  state_change: (e) =>
    isString(e.from) &&
    isString(e.to) &&
    isString(e.reason) &&
    isOptional(e.signal, isString),
  launch_warning: (e) => isString(e.message),
  git: (e) => isString(e.op) && isBoolean(e.ok),
  raw: () => true,
};

/**
 * One `AgentEvent`, or `null` when the fold could not survive it. The cast is
 * the whole point of the function: everything the fold reads has been checked
 * by the time it is made, and an unrecognised `kind` is deliberately let
 * through to `foldEvent`'s forward-compatible default arm.
 */
export function validateAgentEvent(value: unknown): AgentEvent | null {
  if (!isRecord(value)) return null;
  if (!isSeq(value.seq)) return null;
  if (!isString(value.ts)) return null;
  if (!isOptional(value.parent_tool_use_id, isString)) return null;
  if (!isOptional(value.message_id, isString)) return null;
  if (!isString(value.kind)) return null;
  // Keyed by `AgentEventKind`, so a kind added to the union without a check
  // here is a compile error; the annotation is what an unknown `kind` — the
  // whole point of the lookup — reads as at runtime.
  const fields: FieldCheck | undefined =
    EVENT_FIELDS[value.kind as AgentEventKind];
  if (fields !== undefined && !fields(value)) return null;
  return value as unknown as AgentEvent;
}

/** A `Session` far enough to be put on screen; the rest is the server's. */
function validateSession(value: unknown): Session | null {
  if (!isRecord(value)) return null;
  if (!isString(value.id)) return null;
  // The lifecycle enum is only ever added to, so `state` is checked as a
  // string and not against this build's list of names.
  if (!isString(value.state)) return null;
  return value as unknown as Session;
}

/** Validates an already-parsed frame; `parseServerMessage` is the entry point. */
export function validateServerMessage(value: unknown): FrameOutcome {
  if (!isRecord(value)) return bad("not an object");
  const type = value.type;
  if (!isString(type)) return bad("no message type");
  switch (type) {
    case "event": {
      const event = validateAgentEvent(value.event);
      if (event === null) return bad("event does not match the contract");
      return { ok: true, message: { type: "event", event } };
    }
    case "session": {
      const session = validateSession(value.session);
      if (session === null) return bad("session does not match the contract");
      return { ok: true, message: { type: "session", session } };
    }
    case "input_accepted": {
      if (!isString(value.client_id)) return bad("input_accepted: client_id");
      if (!isSeq(value.seq)) return bad("input_accepted: seq");
      return {
        ok: true,
        message: {
          type: "input_accepted",
          client_id: value.client_id,
          seq: value.seq,
        },
      };
    }
    case "input_rejected": {
      if (!isString(value.client_id)) return bad("input_rejected: client_id");
      if (!isString(value.reason)) return bad("input_rejected: reason");
      return {
        ok: true,
        message: {
          type: "input_rejected",
          client_id: value.client_id,
          reason: value.reason,
        },
      };
    }
    case "terminal_closed": {
      if (!isNumber(value.exit_code)) return bad("terminal_closed: exit_code");
      return {
        ok: true,
        message: { type: "terminal_closed", exit_code: value.exit_code },
      };
    }
    case "error": {
      if (!isString(value.message)) return bad("error: message");
      return { ok: true, message: { type: "error", message: value.message } };
    }
    default:
      return { ok: false, unknown: true, detail: "unknown message type" };
  }
}

export function parseServerMessage(data: string): FrameOutcome {
  const value = parseJson(data);
  if (value === undefined) return bad("not JSON");
  return validateServerMessage(value);
}
