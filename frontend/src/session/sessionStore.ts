// The per-session transcript store of `SPEC.md`, "Frontend", "Session state".
//
// Every `AgentEvent` is folded into display messages as it arrives; no field
// ever holds an `AgentEvent[]` (ADR 0022; `ARCHITECTURE.md`, "Frontend
// architecture"). The fold is the pure function `foldEvent`, so the whole
// transcript behaviour is unit-testable from hand-written event sequences
// without a WebSocket.
//
// There is no `pendingPrompt`: the pinned CLI never asks the host a question,
// so there is no `prompt` event and no answer input (ADR 0033).

import { createStore, useStore, type StoreApi } from "zustand";

import type { AgentEvent, Session, SessionInput } from "../types";
import type { SessionState as SessionLifecycleState } from "../types";

/** The name a tool message carries until its `tool_call` is seen. */
const UNKNOWN_TOOL = "unknown";
/** The name a subagent parent carries until its `tool_call` is seen. */
const PLACEHOLDER_AGENT = "Agent";

export interface UserMessage {
  id: string;
  kind: "user";
  text: string;
  /** Optimistic and not yet confirmed by its `user_message` event. */
  pending?: boolean;
  /** Set from `input_rejected`; the message stays visible so it can be resent. */
  rejected?: string;
}

export interface AssistantTextMessage {
  id: string;
  kind: "assistant_text";
  text: string;
  streaming: boolean;
  /**
   * The block stopped without its completing `text`: a park, a kill or a fatal
   * error ended the turn mid-stream, so `text` is the deltas that arrived and
   * not the whole block. A merge needs the difference (`mergeHistory`).
   */
  interrupted?: boolean;
}

export interface ThinkingMessage {
  id: string;
  kind: "thinking";
  text: string;
  redacted: boolean;
}

export interface SubagentInfo {
  description: string;
  agent_type?: string;
  is_error?: boolean;
}

export interface ToolMessage {
  id: string;
  kind: "tool";
  /**
   * The event's `tool_use_id`. Kept on the message so a result, a
   * `subagent_start` and a `tool_call` from an older history page can all be
   * reconciled onto one message.
   */
  tool_use_id: string;
  name: string;
  input: unknown;
  /** `tool_result.content` as delivered; renderers narrow it. */
  result?: unknown;
  is_error?: boolean;
  truncated?: boolean;
  running: boolean;
  /** Message ids nested under this tool when it runs a subagent. */
  children?: string[];
  subagent?: SubagentInfo;
}

export interface SystemMessage {
  id: string;
  kind: "system";
  text: string;
  level: "info" | "warn" | "error";
  detail?: unknown;
}

export interface ResultMessage {
  id: string;
  kind: "result";
  subtype: string;
  is_error: boolean;
  num_turns: number;
  duration_ms: number;
  cost_usd?: number;
  usage?: unknown;
}

export interface RawMessage {
  id: string;
  kind: "raw";
  native: unknown;
}

export type Message =
  | UserMessage
  | AssistantTextMessage
  | ThinkingMessage
  | ToolMessage
  | SystemMessage
  | ResultMessage
  | RawMessage;

export type ConnectionStatus = "connecting" | "live" | "reconnecting";

/**
 * The state shape of `SPEC.md`, "Session state", plus the documented internal
 * cursors the transcript view and the Changes panel need.
 */
export interface SessionState {
  session: Session | null;
  status: ConnectionStatus;
  /** Highest applied `seq`; the socket reconnects with `after = lastSeq`. */
  lastSeq: number;
  /** Top-level message ids in display order. */
  order: string[];
  messages: Record<string, Message>;
  /** `tool_use_id` -> message id awaiting a result. */
  pendingTools: Record<string, string>;
  /** `parent_tool_use_id` -> message ids nested under it. */
  subagents: Record<string, string[]>;
  /** Lowest `seq` held; the REST history cursor (`?before=`). */
  oldestSeq: number | null;
  /** Whether older history remains behind `oldestSeq`. */
  hasMore: boolean;
  /** `seq` of the last `git` event, consumed by the Changes panel. */
  gitEventSeq: number;
  /**
   * `seq` of the *first* event held that closed the streamed blocks — a
   * `result`, a fatal `error` or a `state_change` into an ended state. A merged
   * older page consults it: a page cut inside a delta run ends with a fragment
   * the older fold never saw finish, and the first close after the cut is that
   * block's own end, so a live message above it belongs to the fragment's block
   * and one below it to a later one.
   */
  streamEndSeq: number;
  /** A turn is in progress; the composer's button reads "Interject". */
  turnActive: boolean;
  /**
   * The most recent `input_rejected`. The composer explains it and restores
   * the text from the rejected optimistic message; the next send clears it.
   */
  lastRejection: { client_id: string; reason: string } | null;
}

export interface SessionActions {
  reset: () => void;
  setSession: (session: Session) => void;
  setStatus: (status: ConnectionStatus) => void;
  /** Live and replay path: ignores `seq <= lastSeq`, advances `lastSeq`. */
  applyEvent: (event: AgentEvent) => void;
  /** An older page, newest-last as the REST endpoint returns it. */
  prependHistory: (events: AgentEvent[], hasMore: boolean) => void;
  /**
   * A message the client itself has to show — a socket `error` frame, say —
   * rendered like any other system message but carrying no `seq`, so it never
   * touches the replay cursors.
   */
  addSystemMessage: (
    text: string,
    level?: SystemMessage["level"],
    detail?: unknown,
  ) => void;
  addOptimisticUser: (clientId: string, input: SessionInput) => void;
  inputAccepted: (clientId: string, seq: number) => void;
  inputRejected: (clientId: string, reason: string) => void;
}

export type SessionStore = SessionState & SessionActions;

export function optimisticId(clientId: string): string {
  return `client:${clientId}`;
}

function eventId(seq: number): string {
  return `e${seq}`;
}

function placeholderId(toolUseId: string): string {
  return `tool:${toolUseId}`;
}

export function emptySessionState(): SessionState {
  return {
    session: null,
    status: "connecting",
    lastSeq: 0,
    order: [],
    messages: {},
    pendingTools: {},
    subagents: {},
    oldestSeq: null,
    hasMore: false,
    gitEventSeq: 0,
    streamEndSeq: 0,
    turnActive: false,
    lastRejection: null,
  };
}

function scopeIds(state: SessionState, parent: string | undefined): string[] {
  return parent === undefined ? state.order : (state.subagents[parent] ?? []);
}

function isPlaceholderTool(message: ToolMessage): boolean {
  return message.name === UNKNOWN_TOOL || message.name === PLACEHOLDER_AGENT;
}

/** The message id of the tool message carrying `toolUseId`, if one exists. */
function findToolMessageId(
  state: SessionState,
  toolUseId: string,
): string | undefined {
  const pending = state.pendingTools[toolUseId];
  if (pending !== undefined && state.messages[pending]) return pending;
  for (const message of Object.values(state.messages)) {
    if (message.kind === "tool" && message.tool_use_id === toolUseId) {
      return message.id;
    }
  }
  return undefined;
}

function updateMessage(
  state: SessionState,
  id: string,
  next: Message,
): SessionState {
  return { ...state, messages: { ...state.messages, [id]: next } };
}

/**
 * A subagent whose parent `tool_call` was never seen still needs somewhere to
 * hang its output, so an `Agent` placeholder is created at the top level and
 * merged into by the `tool_call` if one arrives later.
 */
function ensureParentTool(state: SessionState, toolUseId: string): SessionState {
  if (findToolMessageId(state, toolUseId) !== undefined) return state;
  const id = placeholderId(toolUseId);
  const message: ToolMessage = {
    id,
    kind: "tool",
    tool_use_id: toolUseId,
    name: PLACEHOLDER_AGENT,
    input: null,
    running: true,
    children: [],
    subagent: { description: PLACEHOLDER_AGENT },
  };
  return {
    ...state,
    messages: { ...state.messages, [id]: message },
    order: [...state.order, id],
    subagents: { ...state.subagents, [toolUseId]: state.subagents[toolUseId] ?? [] },
    pendingTools: { ...state.pendingTools, [toolUseId]: id },
  };
}

/** Appends a message at the top level, or under `parent` when one is given. */
function placeMessage(
  state: SessionState,
  message: Message,
  parent: string | undefined,
): SessionState {
  if (parent === undefined) {
    return {
      ...state,
      messages: { ...state.messages, [message.id]: message },
      order: [...state.order, message.id],
    };
  }
  const withParent = ensureParentTool(state, parent);
  const messages = { ...withParent.messages, [message.id]: message };
  const parentId = findToolMessageId(withParent, parent);
  if (parentId !== undefined) {
    const parentMessage = messages[parentId];
    if (parentMessage?.kind === "tool") {
      messages[parentId] = {
        ...parentMessage,
        children: [...(parentMessage.children ?? []), message.id],
      };
    }
  }
  const nested = withParent.subagents[parent] ?? [];
  return {
    ...withParent,
    messages,
    subagents: { ...withParent.subagents, [parent]: [...nested, message.id] },
  };
}

/**
 * Whether a message ends a streamed text block. A row the agent did not write
 * — a user message (the "Interject" of a live turn, optimistic or confirmed), a
 * system row from a `git`, `launch_warning`, `state_change` or socket error, a
 * `raw` line — can land in the middle of a delta run without meaning the block
 * ended, so the fold looks past it. A tool, thinking, a `result` or a completed
 * text block does end it.
 */
function endsStreaming(message: Message): boolean {
  return (
    message.kind !== "user" &&
    message.kind !== "system" &&
    message.kind !== "raw"
  );
}

/**
 * The streaming assistant message a `text_delta`/`text` belongs to: the last
 * block-ending message of the scope, and only while it is still streaming.
 * Anything else (a tool, a completed text block) starts a new message, so a
 * delta never attaches to an assistant message from an earlier turn.
 */
function streamingId(
  state: SessionState,
  parent: string | undefined,
): string | undefined {
  const ids = scopeIds(state, parent);
  for (let i = ids.length - 1; i >= 0; i -= 1) {
    const message = state.messages[ids[i]];
    if (!message || !endsStreaming(message)) continue;
    if (message.kind !== "assistant_text") return undefined;
    return message.streaming ? message.id : undefined;
  }
  return undefined;
}

/**
 * Completes every streamed block: the turn is over, whether it ended by itself
 * (`result`), was interrupted (a park or a kill, which produce no `result`) or
 * failed fatally. A block still streaming here never got its completing `text`,
 * so it is marked `interrupted` and its text stays what arrived. The first
 * close is kept in `streamEndSeq`, so an older page merged in later neither
 * brings a blinking cursor back nor mistakes a later block for this one.
 */
function endStreaming(state: SessionState, seq: number): SessionState {
  let messages: Record<string, Message> | null = null;
  for (const message of Object.values(state.messages)) {
    if (message.kind === "assistant_text" && message.streaming) {
      messages ??= { ...state.messages };
      messages[message.id] = { ...message, streaming: false, interrupted: true };
    }
  }
  return {
    ...state,
    messages: messages ?? state.messages,
    streamEndSeq: state.streamEndSeq || seq,
  };
}

function system(
  seq: number,
  text: string,
  level: SystemMessage["level"],
  detail?: unknown,
): SystemMessage {
  return { id: eventId(seq), kind: "system", text, level, detail };
}

function initText(model: string | undefined, resumed: boolean): string {
  const base = resumed ? "Session resumed" : "Session started";
  return model ? `${base} (${model})` : base;
}

function stateChangeText(
  from: SessionLifecycleState,
  to: SessionLifecycleState,
  reason: string,
  signal: "SIGINT" | "SIGTERM" | undefined,
): string {
  if (signal === "SIGINT") return "Stopped (SIGINT)";
  if (signal === "SIGTERM") return "Killed (SIGTERM)";
  return `${from} → ${to}: ${reason}`;
}

/** Applies a `user_message` that confirms an optimistic message, in place. */
function replaceOptimistic(
  state: SessionState,
  clientId: string,
  message: UserMessage,
): SessionState | null {
  const oldId = optimisticId(clientId);
  const index = state.order.indexOf(oldId);
  if (index === -1) return null;
  const order = [...state.order];
  order[index] = message.id;
  const messages = { ...state.messages, [message.id]: message };
  delete messages[oldId];
  return { ...state, order, messages };
}

const TURN_STARTING: ReadonlySet<AgentEvent["kind"]> = new Set([
  "user_message",
  "text_delta",
  "text",
  "thinking",
  "tool_call",
]);

/**
 * Folds one event into the state. Pure: `applyEvent` and `prependHistory`
 * share it, and `lastSeq`/`oldestSeq` bookkeeping stays with the callers.
 */
export function foldEvent(state: SessionState, event: AgentEvent): SessionState {
  const parent = event.parent_tool_use_id;
  const id = eventId(event.seq);
  let next = state;
  if (TURN_STARTING.has(event.kind)) next = { ...next, turnActive: true };

  switch (event.kind) {
    case "init":
      return placeMessage(
        next,
        system(event.seq, initText(event.model, event.resumed), "info", {
          cli_session_id: event.cli_session_id,
          model: event.model,
          tools: event.tools,
          mcp_servers: event.mcp_servers,
        }),
        parent,
      );

    case "user_message": {
      const message: UserMessage = { id, kind: "user", text: event.text };
      if (event.client_id !== undefined) {
        const replaced = replaceOptimistic(next, event.client_id, message);
        if (replaced) return replaced;
      }
      return placeMessage(next, message, parent);
    }

    case "text_delta": {
      const current = streamingId(next, parent);
      if (current !== undefined) {
        const message = next.messages[current];
        if (message?.kind === "assistant_text") {
          return updateMessage(next, current, {
            ...message,
            text: message.text + event.text,
          });
        }
      }
      return placeMessage(
        next,
        { id, kind: "assistant_text", text: event.text, streaming: true },
        parent,
      );
    }

    case "text": {
      const current = streamingId(next, parent);
      if (current !== undefined) {
        const message = next.messages[current];
        if (message?.kind === "assistant_text") {
          // Keeps the streaming message's id so `order` is stable.
          return updateMessage(next, current, {
            ...message,
            text: event.text,
            streaming: false,
          });
        }
      }
      return placeMessage(
        next,
        { id, kind: "assistant_text", text: event.text, streaming: false },
        parent,
      );
    }

    case "thinking":
      return placeMessage(
        next,
        { id, kind: "thinking", text: event.text, redacted: event.redacted },
        parent,
      );

    case "tool_call": {
      const existingId = findToolMessageId(next, event.tool_use_id);
      const existing = existingId ? next.messages[existingId] : undefined;
      if (existingId && existing?.kind === "tool" && isPlaceholderTool(existing)) {
        // A placeholder from `subagent_start` or from an early `tool_result`:
        // one tool message, whichever order the two arrived in.
        const merged: ToolMessage = {
          ...existing,
          name: event.name,
          input: event.input,
        };
        const withMerge = updateMessage(next, existingId, merged);
        return merged.running
          ? {
              ...withMerge,
              pendingTools: {
                ...withMerge.pendingTools,
                [event.tool_use_id]: existingId,
              },
            }
          : withMerge;
      }
      const message: ToolMessage = {
        id,
        kind: "tool",
        tool_use_id: event.tool_use_id,
        name: event.name,
        input: event.input,
        running: true,
      };
      const placed = placeMessage(next, message, parent);
      return {
        ...placed,
        pendingTools: { ...placed.pendingTools, [event.tool_use_id]: id },
      };
    }

    case "tool_result": {
      const targetId = findToolMessageId(next, event.tool_use_id);
      const target = targetId ? next.messages[targetId] : undefined;
      const pendingTools = { ...next.pendingTools };
      delete pendingTools[event.tool_use_id];
      if (targetId && target?.kind === "tool") {
        const completed: ToolMessage = {
          ...target,
          result: event.content,
          is_error: event.is_error,
          truncated: event.truncated,
          running: false,
        };
        return { ...updateMessage(next, targetId, completed), pendingTools };
      }
      // Nothing is dropped: a result without a call becomes an `unknown` tool.
      const message: ToolMessage = {
        id,
        kind: "tool",
        tool_use_id: event.tool_use_id,
        name: UNKNOWN_TOOL,
        input: null,
        result: event.content,
        is_error: event.is_error,
        truncated: event.truncated,
        running: false,
      };
      return { ...placeMessage(next, message, parent), pendingTools };
    }

    case "permission_denied":
      return placeMessage(
        next,
        system(
          event.seq,
          `Permission denied: ${event.name} — ${event.reason}`,
          "warn",
          { tool_use_id: event.tool_use_id },
        ),
        parent,
      );

    case "subagent_start": {
      const withParent = ensureParentTool(next, event.tool_use_id);
      const targetId = findToolMessageId(withParent, event.tool_use_id);
      const target = targetId ? withParent.messages[targetId] : undefined;
      const marked =
        targetId && target?.kind === "tool"
          ? updateMessage(withParent, targetId, {
              ...target,
              children: target.children ?? [],
              subagent: {
                description: event.description,
                agent_type: event.agent_type,
              },
            })
          : withParent;
      return {
        ...marked,
        subagents: {
          ...marked.subagents,
          [event.tool_use_id]: marked.subagents[event.tool_use_id] ?? [],
        },
      };
    }

    case "subagent_end": {
      const withParent = ensureParentTool(next, event.tool_use_id);
      const targetId = findToolMessageId(withParent, event.tool_use_id);
      const target = targetId ? withParent.messages[targetId] : undefined;
      if (!targetId || target?.kind !== "tool") return withParent;
      return updateMessage(withParent, targetId, {
        ...target,
        subagent: {
          description: target.subagent?.description ?? PLACEHOLDER_AGENT,
          agent_type: target.subagent?.agent_type,
          is_error: event.is_error,
        },
      });
    }

    case "result": {
      const message: ResultMessage = {
        id,
        kind: "result",
        subtype: event.subtype,
        is_error: event.is_error,
        num_turns: event.num_turns,
        duration_ms: event.duration_ms,
        cost_usd: event.cost_usd,
        usage: event.usage,
      };
      const placed = placeMessage(endStreaming(next, event.seq), message, parent);
      return { ...placed, turnActive: false };
    }

    case "error": {
      const text = event.fatal ? `${event.message} (fatal)` : event.message;
      // A fatal error ends the turn wherever the stream had got to, so the
      // cursor of a half-written block stops blinking (there is no `result`).
      const placed = placeMessage(
        event.fatal ? endStreaming(next, event.seq) : next,
        system(event.seq, text, "error"),
        parent,
      );
      return event.fatal ? { ...placed, turnActive: false } : placed;
    }

    case "state_change": {
      const text = stateChangeText(
        event.from,
        event.to,
        event.reason,
        event.signal,
      );
      const ended =
        event.to === "parked" || event.to === "done" || event.to === "failed";
      // A kill or a park mid-stream never produces a `result`, so the streamed
      // block that was interrupted is completed here instead.
      const placed = placeMessage(
        ended ? endStreaming(next, event.seq) : next,
        system(event.seq, text, "info", {
          from: event.from,
          to: event.to,
          reason: event.reason,
          signal: event.signal,
        }),
        parent,
      );
      const session = placed.session
        ? { ...placed.session, state: event.to }
        : null;
      return {
        ...placed,
        session,
        turnActive: ended ? false : placed.turnActive,
      };
    }

    case "launch_warning":
      return placeMessage(
        next,
        system(event.seq, event.message, "warn"),
        parent,
      );

    case "git": {
      const text = event.ok
        ? `Git ${event.op} succeeded`
        : `Git ${event.op} failed`;
      const placed = placeMessage(
        next,
        system(event.seq, text, event.ok ? "info" : "warn", event.detail),
        parent,
      );
      return { ...placed, gitEventSeq: event.seq };
    }

    case "raw":
      return placeMessage(next, { id, kind: "raw", native: event.native }, parent);
  }
}

/** Keeps the first occurrence of each id and drops everything merged away. */
function compact(ids: string[], dropped: ReadonlySet<string>): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const id of ids) {
    if (dropped.has(id) || seen.has(id)) continue;
    seen.add(id);
    out.push(id);
  }
  return out;
}

/**
 * The subagent marking of a merged pair: whichever side saw the
 * `subagent_start` describes it, whichever side saw the `subagent_end` decides
 * `is_error`.
 */
function mergeSubagentInfo(
  older: SubagentInfo | undefined,
  live: SubagentInfo | undefined,
): SubagentInfo | undefined {
  if (!older && !live) return undefined;
  const described =
    older && older.description !== PLACEHOLDER_AGENT
      ? older
      : live && live.description !== PLACEHOLDER_AGENT
        ? live
        : (older ?? live);
  return {
    description: described?.description ?? PLACEHOLDER_AGENT,
    agent_type: described?.agent_type,
    is_error: older?.is_error ?? live?.is_error,
  };
}

/**
 * One tool message out of two halves of the same `tool_use_id`: the older
 * message keeps its id and position, the real `tool_call` wins over a
 * placeholder, and the completing half contributes the result.
 */
function mergeToolMessages(older: ToolMessage, live: ToolMessage): ToolMessage {
  const call = isPlaceholderTool(older) && !isPlaceholderTool(live) ? live : older;
  const completed = !older.running ? older : !live.running ? live : undefined;
  const children =
    older.children || live.children
      ? [...(older.children ?? []), ...(live.children ?? [])]
      : undefined;
  return {
    ...older,
    name: call.name,
    input: call.input,
    result: completed?.result,
    is_error: completed?.is_error,
    truncated: completed?.truncated,
    running: completed === undefined,
    children,
    subagent: mergeSubagentInfo(older.subagent, live.subagent),
  };
}

/** The scopes a merge has to reconcile: the top level and every subagent. */
function mergedScopes(
  older: SessionState,
  live: SessionState,
): (string | undefined)[] {
  const keys = new Set([
    ...Object.keys(older.subagents),
    ...Object.keys(live.subagents),
  ]);
  return [undefined, ...keys];
}

/** The last message of a scope that ends a streamed block, if there is one. */
function lastBlockMessage(
  state: SessionState,
  parent: string | undefined,
): Message | undefined {
  const ids = scopeIds(state, parent);
  for (let i = ids.length - 1; i >= 0; i -= 1) {
    const message = state.messages[ids[i]];
    if (message && endsStreaming(message)) return message;
  }
  return undefined;
}

/** The first message of a scope that ends a streamed block, if there is one. */
function firstBlockMessage(
  state: SessionState,
  parent: string | undefined,
): Message | undefined {
  for (const id of scopeIds(state, parent)) {
    const message = state.messages[id];
    if (message && endsStreaming(message)) return message;
  }
  return undefined;
}

/** The `seq` an event-derived message carries in its id, if it has one. */
function messageSeq(id: string): number | undefined {
  const match = /^e(\d+)$/.exec(id);
  return match ? Number(match[1]) : undefined;
}

/**
 * Whether the live half's first block message continues the older page's
 * fragment rather than starting a block of its own. The first close held by the
 * live half ends the fragment's block, so a message above it is the same block
 * and one below it belongs to a later turn — which is what a park and its
 * resume leave behind, with only transparent system and user rows between.
 */
function continuesBlock(live: SessionState, head: Message): boolean {
  if (live.streamEndSeq === 0) return true;
  const seq = messageSeq(head.id);
  return seq === undefined || seq < live.streamEndSeq;
}

/**
 * Reconciles a page boundary that fell inside a delta run. The older page was
 * folded on its own, so it ends with a streaming fragment of a block the live
 * half has already rebuilt from the later deltas. The two halves become one
 * message at the older position, exactly as a split tool does. What the live
 * half contributes depends on how its own half ended: a head still streaming,
 * or one an interruption closed without a `text`, holds only the deltas after
 * the cut and is appended to the fragment; one completed by `text` holds the
 * whole block and replaces it. A fragment whose block ended with nothing more
 * in its scope — a park with no further delta — merely stops streaming, and so
 * does one whose live scope opens with a tool or with a later block. Writes
 * into `messages` and records what it merged away in `dropped`, which the
 * caller compacts out of every order and child list.
 */
function mergeStreamingFragments(
  older: SessionState,
  live: SessionState,
  messages: Record<string, Message>,
  dropped: Set<string>,
): void {
  const interrupt = (fragment: AssistantTextMessage) => {
    messages[fragment.id] = { ...fragment, streaming: false, interrupted: true };
  };

  for (const parent of mergedScopes(older, live)) {
    const fragment = lastBlockMessage(older, parent);
    if (fragment?.kind !== "assistant_text" || !fragment.streaming) continue;
    const head = firstBlockMessage(live, parent);
    if (head === undefined) {
      // Only user, system or raw rows follow. The fragment is still the tail of
      // the live scope, so the next delta continues it — unless the live half
      // saw the turn end, which is what interrupted the block.
      if (live.streamEndSeq > 0) interrupt(fragment);
      continue;
    }
    if (head.kind !== "assistant_text" || !continuesBlock(live, head)) {
      interrupt(fragment);
      continue;
    }
    // Only a head completed by its `text` carries the block's whole text
    // (`SPEC.md`, "AgentEvent"); a streaming or interrupted one carries just
    // the deltas that arrived after the cut.
    const partial = head.streaming || head.interrupted === true;
    messages[fragment.id] = {
      ...fragment,
      text: partial ? fragment.text + head.text : head.text,
      streaming: head.streaming,
      interrupted: head.interrupted,
    };
    dropped.add(head.id);
  }
}

/**
 * Merges an older page folded on its own into the live state. The two orders
 * are concatenated, and every placeholder tool message the live half had to
 * invent — an `unknown` for a result without a call, an `Agent` for a subagent
 * whose parent `tool_call` had scrolled out of view — is reconciled onto the
 * real message from the older page so one `tool_use_id` is one message, and a
 * streaming fragment the page boundary cut out of a delta run is joined to or
 * dropped in favour of the live half of the same block.
 */
function mergeHistory(older: SessionState, live: SessionState): SessionState {
  const messages: Record<string, Message> = { ...older.messages, ...live.messages };
  const subagents: Record<string, string[]> = { ...older.subagents };
  for (const [key, ids] of Object.entries(live.subagents)) {
    subagents[key] = [...(subagents[key] ?? []), ...ids];
  }
  const pendingTools = { ...older.pendingTools, ...live.pendingTools };
  const dropped = new Set<string>();

  for (const liveMessage of Object.values(live.messages)) {
    if (liveMessage.kind !== "tool" || !isPlaceholderTool(liveMessage)) continue;
    const olderId = findToolMessageId(older, liveMessage.tool_use_id);
    if (olderId === undefined) continue;
    const olderMessage = older.messages[olderId];
    if (olderMessage?.kind !== "tool") continue;

    const merged = mergeToolMessages(olderMessage, liveMessage);
    messages[olderId] = merged;
    // Both halves may have invented the same `tool:<id>` placeholder, in which
    // case there is nothing to drop and only the duplicate entries to compact.
    if (liveMessage.id !== olderId) dropped.add(liveMessage.id);
    if (merged.running) {
      pendingTools[merged.tool_use_id] = olderId;
    } else {
      delete pendingTools[merged.tool_use_id];
    }
  }

  mergeStreamingFragments(older, live, messages, dropped);

  // Nothing may keep pointing at a merged-away message: not `messages`, not
  // `order`, not a subagent's list, not the `children` of the tool it was
  // nested under.
  for (const id of dropped) delete messages[id];
  const order = compact([...older.order, ...live.order], dropped);
  for (const [key, ids] of Object.entries(subagents)) {
    subagents[key] = compact(ids, dropped);
  }
  for (const message of Object.values(messages)) {
    if (message.kind !== "tool" || !message.children) continue;
    messages[message.id] = { ...message, children: compact(message.children, dropped) };
  }
  for (const [toolUseId, id] of Object.entries(pendingTools)) {
    if (dropped.has(id)) delete pendingTools[toolUseId];
  }

  return {
    ...live,
    messages,
    order,
    subagents,
    pendingTools,
    gitEventSeq: live.gitEventSeq || older.gitEventSeq,
    // The earliest close held: the older page's, when it has one.
    streamEndSeq: older.streamEndSeq || live.streamEndSeq,
    // The first page is loaded into an empty store, so its tail decides whether
    // a turn is in progress; later pages never revise the live answer.
    turnActive:
      live.lastSeq === 0 && live.order.length === 0
        ? older.turnActive
        : live.turnActive,
  };
}

export function createSessionStore(): StoreApi<SessionStore> {
  // Ids for messages the client invents; `e<seq>` is reserved for events.
  let local = 0;
  // `turnActive` as each optimistic send found it, so a rejection can put it
  // back: a send that the orchestrator refuses never started a turn, and the
  // composer's button must go back to reading "Send".
  // The `lastSeq` it was sent at comes with it: an event that arrived since
  // may have started a turn of its own, and that turn is not this send's to
  // undo.
  const turnActiveBefore = new Map<string, { active: boolean; seq: number }>();
  return createStore<SessionStore>()((set, get) => ({
    ...emptySessionState(),

    reset: () => {
      turnActiveBefore.clear();
      set(emptySessionState());
    },

    setSession: (session) => {
      set({ session });
    },

    setStatus: (status) => {
      set({ status });
    },

    applyEvent: (event) => {
      set((state) => {
        // Reconnect replay: the socket resends from `after`, so anything at or
        // below `lastSeq` has already been folded.
        if (event.seq <= state.lastSeq) return state;
        const next = foldEvent(state, event);
        return {
          ...next,
          lastSeq: event.seq,
          oldestSeq: next.oldestSeq ?? event.seq,
        };
      });
    },

    prependHistory: (events, hasMore) => {
      set((state) => {
        const accepted = events.filter(
          (event) => state.oldestSeq === null || event.seq < state.oldestSeq,
        );
        if (accepted.length === 0) return { ...state, hasMore };
        let older = emptySessionState();
        for (const event of accepted) older = foldEvent(older, event);
        const merged = mergeHistory(older, state);
        const oldest = Math.min(...accepted.map((event) => event.seq));
        return {
          ...merged,
          hasMore,
          oldestSeq: oldest,
          lastSeq: Math.max(
            state.lastSeq,
            ...accepted.map((event) => event.seq),
          ),
        };
      });
    },

    addSystemMessage: (text, level = "error", detail) => {
      set((state) => {
        local += 1;
        const id = `local:${local}`;
        const message: SystemMessage = { id, kind: "system", text, level, detail };
        return {
          messages: { ...state.messages, [id]: message },
          order: [...state.order, id],
        };
      });
    },

    addOptimisticUser: (clientId, input) => {
      if (!turnActiveBefore.has(clientId)) {
        const { turnActive, lastSeq } = get();
        turnActiveBefore.set(clientId, { active: turnActive, seq: lastSeq });
      }
      set((state) => {
        const id = optimisticId(clientId);
        const message: UserMessage = {
          id,
          kind: "user",
          text: input.text,
          pending: true,
        };
        return {
          messages: { ...state.messages, [id]: message },
          order: [...state.order, id],
          turnActive: true,
          // A new attempt supersedes the rejection the composer is explaining.
          lastRejection: null,
        };
      });
    },

    inputAccepted: (clientId) => {
      // Accepted: the turn this send started is the live one, so there is
      // nothing left to restore.
      turnActiveBefore.delete(clientId);
      set((state) => {
        const id = optimisticId(clientId);
        const message = state.messages[id];
        if (message?.kind !== "user") return state;
        // Acceptance is not delivery (ADR 0020): the message stays pending
        // until its `user_message` event replaces it.
        return {
          messages: {
            ...state.messages,
            [id]: { ...message, pending: true, rejected: undefined },
          },
        };
      });
    },

    inputRejected: (clientId, reason) => {
      const before = turnActiveBefore.get(clientId);
      turnActiveBefore.delete(clientId);
      set((state) => {
        const lastRejection = { client_id: clientId, reason };
        const id = optimisticId(clientId);
        const message = state.messages[id];
        // A rejection of something this client never sent optimistically —
        // another browser's message, or one sent before a reload — still has
        // to reach the composer, so the rejection is recorded either way.
        if (message?.kind !== "user") return { ...state, lastRejection };
        // Another send is still in flight, or an event has arrived since:
        // only this send's own optimism is undone.
        const othersPending = Object.values(state.messages).some(
          (other) => other.kind === "user" && other.pending && other.id !== id,
        );
        const restore =
          before !== undefined && !othersPending && before.seq === state.lastSeq;
        return {
          messages: {
            ...state.messages,
            [id]: { ...message, pending: false, rejected: reason },
          },
          turnActive: restore ? before.active : state.turnActive,
          lastRejection,
        };
      });
    },
  }));
}

// One store per mounted session id, so navigating between sessions does not
// leak folded state. `useSessionSocket` calls `reset()` when the id changes.
const stores = new Map<string, StoreApi<SessionStore>>();

export function getSessionStore(sessionId: string): StoreApi<SessionStore> {
  let store = stores.get(sessionId);
  if (!store) {
    store = createSessionStore();
    stores.set(sessionId, store);
  }
  return store;
}

export function disposeSessionStore(sessionId: string): void {
  stores.delete(sessionId);
}

/** Subscribes to the store bound to `sessionId`. */
export function useSessionStore<T>(
  sessionId: string,
  selector: (state: SessionStore) => T,
): T {
  return useStore(getSessionStore(sessionId), selector);
}
