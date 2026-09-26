// The session socket's text-frame boundary (`SPEC.md`, "WebSocket: session
// stream", "AgentEvent"): what is accepted, what is rejected without touching
// the transcript, and what a frame from a newer orchestrator does.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import * as auth from "../services/auth";
import * as sessions from "../services/sessions";
import type { AgentEvent } from "../types";
import { parseServerMessage, validateAgentEvent } from "./serverMessage";
import {
  disposeSessionStore,
  foldEvent,
  getSessionStore,
} from "./sessionStore";
import type { SocketLike, TerminalFrame } from "./useSessionSocket";
import { SessionSocket } from "./useSessionSocket";

vi.mock("../services/auth", () => ({
  getAccessToken: vi.fn(() => "token-one"),
  // The store registry reads the current principal when it creates a store;
  // this suite signs nobody in.
  getCurrentUser: vi.fn(() => null),
  refreshAccessToken: vi.fn(),
  onCredentialsReplaced: vi.fn(() => () => {}),
  onSignOut: vi.fn(() => () => {}),
  isStaleRefreshError: vi.fn(() => false),
}));

vi.mock("../services/sessions", () => ({
  getSession: vi.fn(),
  listEvents: vi.fn(),
  sendInput: vi.fn(),
  stopSession: vi.fn(),
}));

const SESSION_ID = "33333333-3333-4333-8333-333333333333";

/** A minimal well-formed event, for the tests that spoil one field of it. */
function event(
  overrides: Record<string, unknown> = {},
): Record<string, unknown> {
  return {
    seq: 1,
    ts: "2026-09-21T10:00:00Z",
    kind: "text",
    text: "hi",
    ...overrides,
  };
}

function frame(message: unknown): string {
  return JSON.stringify(message);
}

describe("validateAgentEvent", () => {
  it("accepts a well-formed event", () => {
    expect(validateAgentEvent(event())).not.toBeNull();
  });

  it("rejects a missing or non-numeric seq", () => {
    expect(validateAgentEvent(event({ seq: undefined }))).toBeNull();
    expect(validateAgentEvent(event({ seq: null }))).toBeNull();
    expect(validateAgentEvent(event({ seq: "4" }))).toBeNull();
    expect(validateAgentEvent(event({ seq: 1.5 }))).toBeNull();
    expect(validateAgentEvent(event({ seq: 0 }))).toBeNull();
    expect(
      validateAgentEvent(event({ seq: Number.MAX_SAFE_INTEGER + 2 })),
    ).toBeNull();
  });

  it("rejects a non-object and a null", () => {
    expect(validateAgentEvent(null)).toBeNull();
    expect(validateAgentEvent([event()])).toBeNull();
    expect(validateAgentEvent("event")).toBeNull();
  });

  it("rejects a field the fold reads being of the wrong type", () => {
    expect(validateAgentEvent(event({ text: 7 }))).toBeNull();
    expect(
      validateAgentEvent(
        event({ kind: "thinking", text: "t", redacted: null }),
      ),
    ).toBeNull();
    expect(
      validateAgentEvent(event({ kind: "tool_call", tool_use_id: "t1" })),
    ).toBeNull();
    expect(
      validateAgentEvent(
        event({
          kind: "init",
          cli_session_id: "c",
          tools: [1],
          mcp_servers: [],
          resumed: true,
        }),
      ),
    ).toBeNull();
    expect(
      validateAgentEvent(
        event({
          kind: "init",
          cli_session_id: "c",
          tools: [],
          mcp_servers: [{ name: "b" }],
          resumed: true,
        }),
      ),
    ).toBeNull();
  });

  it("rejects an optional field present with the wrong type", () => {
    expect(validateAgentEvent(event({ parent_tool_use_id: 3 }))).toBeNull();
    expect(
      validateAgentEvent(
        event({ kind: "user_message", text: "a", client_id: 9 }),
      ),
    ).toBeNull();
  });

  it("accepts a kind it has never heard of", () => {
    // Event kinds are only added, and a tab outlives an upgrade.
    const accepted = validateAgentEvent(
      event({ kind: "sandbox_warning", detail: "x" }),
    );
    expect(accepted).not.toBeNull();
    expect(accepted?.seq).toBe(1);
  });

  it("still requires the envelope of an unknown kind", () => {
    expect(validateAgentEvent({ kind: "sandbox_warning", ts: "t" })).toBeNull();
    expect(validateAgentEvent({ kind: 4, seq: 1, ts: "t" })).toBeNull();
  });
});

describe("parseServerMessage", () => {
  it("rejects text that is not JSON", () => {
    const outcome = parseServerMessage("{not json");
    expect(outcome.ok).toBe(false);
    expect(outcome.ok === false && outcome.unknown).toBe(false);
  });

  it("rejects valid JSON of the wrong shape", () => {
    expect(parseServerMessage("[]").ok).toBe(false);
    expect(parseServerMessage("null").ok).toBe(false);
    expect(parseServerMessage('"event"').ok).toBe(false);
    expect(parseServerMessage(frame({ event: event() })).ok).toBe(false);
    expect(parseServerMessage(frame({ type: 3 })).ok).toBe(false);
  });

  it("rejects a known type with a missing or null field", () => {
    expect(parseServerMessage(frame({ type: "error" })).ok).toBe(false);
    expect(parseServerMessage(frame({ type: "error", message: null })).ok).toBe(
      false,
    );
    expect(
      parseServerMessage(frame({ type: "input_accepted", client_id: "c" })).ok,
    ).toBe(false);
    expect(
      parseServerMessage(
        frame({ type: "input_accepted", client_id: "c", seq: "3" }),
      ).ok,
    ).toBe(false);
    expect(
      parseServerMessage(frame({ type: "input_rejected", client_id: "c" })).ok,
    ).toBe(false);
    expect(parseServerMessage(frame({ type: "terminal_closed" })).ok).toBe(
      false,
    );
    expect(
      parseServerMessage(frame({ type: "session", session: null })).ok,
    ).toBe(false);
    expect(
      parseServerMessage(frame({ type: "session", session: { id: "s" } })).ok,
    ).toBe(false);
  });

  it("accepts each message of the contract", () => {
    expect(
      parseServerMessage(frame({ type: "event", event: event() })).ok,
    ).toBe(true);
    expect(
      parseServerMessage(
        frame({ type: "session", session: { id: "s", state: "running" } }),
      ).ok,
    ).toBe(true);
    expect(
      parseServerMessage(
        frame({ type: "input_accepted", client_id: "c", seq: 4 }),
      ).ok,
    ).toBe(true);
    expect(
      parseServerMessage(
        frame({ type: "input_rejected", client_id: "c", reason: "no" }),
      ).ok,
    ).toBe(true);
    expect(
      parseServerMessage(frame({ type: "terminal_closed", exit_code: -1 })).ok,
    ).toBe(true);
    expect(
      parseServerMessage(frame({ type: "error", message: "boom" })).ok,
    ).toBe(true);
  });

  it("accepts a session state this build cannot name", () => {
    // The lifecycle enum is only added to.
    expect(
      parseServerMessage(
        frame({ type: "session", session: { id: "s", state: "hibernating" } }),
      ).ok,
    ).toBe(true);
  });

  it("marks an unknown message type as unknown rather than malformed", () => {
    const outcome = parseServerMessage(
      frame({ type: "quota_warning", used: 3 }),
    );
    expect(outcome.ok).toBe(false);
    expect(outcome.ok === false && outcome.unknown).toBe(true);
  });

  it("never puts a field value in its diagnostic", () => {
    const outcome = parseServerMessage(
      frame({ type: "error", message: { secret: "hunter2" } }),
    );
    expect(outcome.ok).toBe(false);
    expect(outcome.ok === false && outcome.detail).not.toContain("hunter2");
  });
});

describe("foldEvent, unknown kind", () => {
  it("renders it as a raw row rather than returning nothing", () => {
    const unknown = validateAgentEvent(
      event({ seq: 7, kind: "sandbox_warning", message: "no network" }),
    );
    expect(unknown).not.toBeNull();
    const store = getSessionStore(SESSION_ID);
    store.getState().applyEvent(unknown as AgentEvent);
    const state = store.getState();
    expect(state.order).toHaveLength(1);
    expect(state.messages[state.order[0] ?? ""]?.kind).toBe("raw");
    // The cursor moves over it: a fold that dropped it would replay the gap on
    // every reconnect.
    expect(state.lastSeq).toBe(7);
    expect(state.oldestSeq).toBe(7);
    disposeSessionStore(SESSION_ID);
  });

  it("leaves a streamed block alone", () => {
    // `raw` rows are transparent to a streamed assistant block, so an unknown
    // kind arriving mid-stream does not split it.
    let state = foldEvent(
      getSessionStore(SESSION_ID).getState(),
      validateAgentEvent(
        event({ seq: 1, kind: "text_delta", text: "he" }),
      ) as AgentEvent,
    );
    state = foldEvent(
      state,
      validateAgentEvent(
        event({ seq: 2, kind: "sandbox_warning" }),
      ) as AgentEvent,
    );
    state = foldEvent(
      state,
      validateAgentEvent(
        event({ seq: 3, kind: "text_delta", text: "llo" }),
      ) as AgentEvent,
    );
    const texts = state.order
      .map((id) => state.messages[id])
      .filter((message) => message?.kind === "assistant_text");
    expect(texts).toHaveLength(1);
    expect(texts[0]?.kind === "assistant_text" && texts[0].text).toBe("hello");
    disposeSessionStore(SESSION_ID);
  });
});

describe("SessionSocket frame handling", () => {
  class FakeSocket implements SocketLike {
    static last: FakeSocket | null = null;

    binaryType: BinaryType = "blob";
    readyState = 0;
    onopen: ((ev: Event) => void) | null = null;
    onmessage: ((ev: MessageEvent) => void) | null = null;
    onclose: ((ev: CloseEvent) => void) | null = null;
    onerror: ((ev: Event) => void) | null = null;

    constructor() {
      FakeSocket.last = this;
    }

    send(): void {}
    close(): void {
      this.readyState = 3;
    }

    accept(): void {
      this.readyState = 1;
      this.onopen?.(new Event("open"));
    }

    deliver(data: unknown): void {
      this.onmessage?.(new MessageEvent("message", { data }));
    }
  }

  let socket: SessionSocket;
  let warn: ReturnType<typeof vi.spyOn>;

  beforeEach(async () => {
    vi.mocked(sessions.listEvents).mockResolvedValue({
      events: [],
      has_more: false,
    });
    warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    vi.spyOn(console, "debug").mockImplementation(() => {});
    socket = new SessionSocket(SESSION_ID, () => new FakeSocket());
    await socket.start();
    FakeSocket.last?.accept();
  });

  afterEach(() => {
    socket.dispose();
    disposeSessionStore(SESSION_ID);
    vi.restoreAllMocks();
    vi.mocked(auth.getAccessToken).mockReturnValue("token-one");
  });

  it("leaves the transcript and the cursor alone for a malformed frame", () => {
    const fake = FakeSocket.last;
    fake?.deliver("{oops");
    fake?.deliver(
      frame({ type: "event", event: { seq: 9, ts: 5, kind: "text" } }),
    );
    fake?.deliver(
      frame({
        type: "event",
        event: { seq: "9", ts: "t", kind: "text", text: "x" },
      }),
    );
    const state = getSessionStore(SESSION_ID).getState();
    expect(state.order).toHaveLength(0);
    expect(state.lastSeq).toBe(0);
    expect(warn).toHaveBeenCalled();
  });

  it("ignores an unknown message type without warning about it", () => {
    FakeSocket.last?.deliver(frame({ type: "quota_warning" }));
    expect(getSessionStore(SESSION_ID).getState().order).toHaveLength(0);
    expect(warn).not.toHaveBeenCalled();
  });

  it("still delivers binary terminal frames", () => {
    const frames: TerminalFrame[] = [];
    socket.terminal.subscribe((received) => frames.push(received));
    const bytes = new Uint8Array([104, 105]);
    FakeSocket.last?.deliver(bytes.slice().buffer);
    FakeSocket.last?.deliver(frame({ type: "terminal_closed", exit_code: 0 }));
    expect(frames).toHaveLength(2);
    expect(frames[0]).toBeInstanceOf(Uint8Array);
    expect(frames[1]).toEqual({ exit_code: 0 });
  });

  it("applies a valid event", () => {
    FakeSocket.last?.deliver(
      frame({
        type: "event",
        event: event({ seq: 3, kind: "text", text: "done" }),
      }),
    );
    const state = getSessionStore(SESSION_ID).getState();
    expect(state.lastSeq).toBe(3);
    expect(state.order).toHaveLength(1);
  });
});
