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
  /** A turn is in progress; the composer's button reads "Interject". */
  turnActive: boolean;
}

export interface SessionActions {
  reset: () => void;
  setSession: (session: Session) => void;
  setStatus: (status: ConnectionStatus) => void;
  /** Live and replay path: ignores `seq <= lastSeq`, advances `lastSeq`. */
  applyEvent: (event: AgentEvent) => void;
  /** An older page, newest-last as the REST endpoint returns it. */
  prependHistory: (events: AgentEvent[], hasMore: boolean) => void;
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
    turnActive: false,
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
 * The streaming assistant message a `text_delta`/`text` belongs to: the last
 * message of the scope, and only while it is still streaming. Anything else
 * (a tool, a completed text block) starts a new message, so a delta never
 * attaches to an assistant message from an earlier turn.
 */
function streamingId(
  state: SessionState,
  parent: string | undefined,
): string | undefined {
  const ids = scopeIds(state, parent);
  for (let i = ids.length - 1; i >= 0; i -= 1) {
    const message = state.messages[ids[i]];
    if (!message) continue;
    if (message.kind !== "assistant_text") return undefined;
    return message.streaming ? message.id : undefined;
  }
  return undefined;
}

function clearStreaming(state: SessionState): SessionState {
  let messages: Record<string, Message> | null = null;
  for (const message of Object.values(state.messages)) {
    if (message.kind === "assistant_text" && message.streaming) {
      messages ??= { ...state.messages };
      messages[message.id] = { ...message, streaming: false };
    }
  }
  return messages ? { ...state, messages } : state;
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
      const placed = placeMessage(clearStreaming(next), message, parent);
      return { ...placed, turnActive: false };
    }

    case "error": {
      const text = event.fatal ? `${event.message} (fatal)` : event.message;
      const placed = placeMessage(next, system(event.seq, text, "error"), parent);
      return event.fatal ? { ...placed, turnActive: false } : placed;
    }

    case "state_change": {
      const text = stateChangeText(
        event.from,
        event.to,
        event.reason,
        event.signal,
      );
      const placed = placeMessage(
        next,
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
      const ended =
        event.to === "parked" || event.to === "done" || event.to === "failed";
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

/**
 * Merges an older page folded on its own into the live state: the two orders
 * are concatenated and any `unknown` tool message the live half created for a
 * result whose `tool_call` sits in the older page is reconciled onto it.
 */
function mergeHistory(older: SessionState, live: SessionState): SessionState {
  const messages: Record<string, Message> = { ...older.messages, ...live.messages };
  let order = [...older.order, ...live.order];
  const subagents: Record<string, string[]> = { ...older.subagents };
  for (const [key, ids] of Object.entries(live.subagents)) {
    subagents[key] = [...(subagents[key] ?? []), ...ids];
  }
  const pendingTools = { ...older.pendingTools, ...live.pendingTools };

  for (const [toolUseId, olderId] of Object.entries(older.pendingTools)) {
    const duplicate = Object.values(live.messages).find(
      (message) =>
        message.kind === "tool" &&
        message.tool_use_id === toolUseId &&
        message.name === UNKNOWN_TOOL,
    );
    const call = messages[olderId];
    if (!duplicate || duplicate.kind !== "tool" || call?.kind !== "tool") continue;
    messages[olderId] = {
      ...call,
      result: duplicate.result,
      is_error: duplicate.is_error,
      truncated: duplicate.truncated,
      running: false,
    };
    delete messages[duplicate.id];
    order = order.filter((entry) => entry !== duplicate.id);
    for (const [key, ids] of Object.entries(subagents)) {
      subagents[key] = ids.filter((entry) => entry !== duplicate.id);
    }
    delete pendingTools[toolUseId];
  }

  return {
    ...live,
    messages,
    order,
    subagents,
    pendingTools,
    gitEventSeq: live.gitEventSeq || older.gitEventSeq,
  };
}

export function createSessionStore(): StoreApi<SessionStore> {
  return createStore<SessionStore>()((set) => ({
    ...emptySessionState(),

    reset: () => {
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

    addOptimisticUser: (clientId, input) => {
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
        };
      });
    },

    inputAccepted: (clientId) => {
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
      set((state) => {
        const id = optimisticId(clientId);
        const message = state.messages[id];
        if (message?.kind !== "user") return state;
        return {
          messages: {
            ...state.messages,
            [id]: { ...message, pending: false, rejected: reason },
          },
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
