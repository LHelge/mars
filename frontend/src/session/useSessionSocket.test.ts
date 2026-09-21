import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ApiError } from "../services/apiClient";
import * as auth from "../services/auth";
import * as sessions from "../services/sessions";
import type { AgentEvent, AuthResponse, Session, SessionState } from "../types";
import { disposeSessionStore, getSessionStore } from "./sessionStore";
import type { SocketLike } from "./useSessionSocket";
import { SessionSocket } from "./useSessionSocket";

vi.mock("../services/auth", async (original) => {
  const actual = await original<typeof auth>();
  return {
    getAccessToken: vi.fn(() => "token-one"),
    refreshAccessToken: vi.fn(),
    onCredentialsReplaced: vi.fn(() => () => {}),
    onSignOut: vi.fn(() => () => {}),
    // The stale-completion marker is a value, not a collaborator: the socket
    // has to recognise the real one.
    StaleRefreshError: actual.StaleRefreshError,
    isStaleRefreshError: actual.isStaleRefreshError,
  };
});

vi.mock("../services/sessions", () => ({
  listEvents: vi.fn(),
  sendInput: vi.fn(),
  stopSession: vi.fn(),
}));

const getAccessToken = vi.mocked(auth.getAccessToken);
const refreshAccessToken = vi.mocked(auth.refreshAccessToken);
const onCredentialsReplaced = vi.mocked(auth.onCredentialsReplaced);
const listEvents = vi.mocked(sessions.listEvents);
const sendInput = vi.mocked(sessions.sendInput);
const stopSession = vi.mocked(sessions.stopSession);

const SESSION_ID = "11111111-1111-4111-8111-111111111111";

class FakeSocket implements SocketLike {
  static instances: FakeSocket[] = [];

  binaryType: BinaryType = "blob";
  readyState = 0;
  readonly url: string;
  readonly sent: (string | ArrayBuffer)[] = [];
  readonly closed: number[] = [];
  /** Store order length at the moment each frame was handed over. */
  readonly orderAtSend: number[] = [];

  onopen: ((ev: Event) => void) | null = null;
  onmessage: ((ev: MessageEvent) => void) | null = null;
  onclose: ((ev: CloseEvent) => void) | null = null;
  onerror: ((ev: Event) => void) | null = null;

  constructor(url: string) {
    this.url = url;
    FakeSocket.instances.push(this);
  }

  send(data: string | ArrayBuffer): void {
    this.sent.push(data);
    this.orderAtSend.push(getSessionStore(SESSION_ID).getState().order.length);
  }

  close(code = 1000): void {
    this.readyState = 3;
    this.closed.push(code);
  }

  /** The server accepting the upgrade. */
  accept(): void {
    this.readyState = 1;
    this.onopen?.(new Event("open"));
  }

  text(message: unknown): void {
    this.onmessage?.(new MessageEvent("message", { data: JSON.stringify(message) }));
  }

  binary(bytes: Uint8Array): void {
    this.onmessage?.(
      new MessageEvent("message", { data: bytes.slice().buffer }),
    );
  }

  serverClose(code: number): void {
    this.readyState = 3;
    this.onclose?.(new CloseEvent("close", { code }));
  }

  get frames(): string[] {
    return this.sent.filter((data): data is string => typeof data === "string");
  }
}

const factory = (url: string): SocketLike => new FakeSocket(url);

function textEvent(seq: number, text: string): AgentEvent {
  return { seq, ts: "2026-01-01T00:00:00Z", kind: "text", text };
}

function session(state: SessionState): Session {
  return {
    id: SESSION_ID,
    project_id: "22222222-2222-4222-8222-222222222222",
    profile_id: "33333333-3333-4333-8333-333333333333",
    kind: "conversational",
    created_by: null,
    title: "Fake session",
    task_id: null,
    handoff_id: null,
    state,
    base_ref: "main",
    branch: "sessions/fake",
    container_id: null,
    cli_session_id: null,
    last_seq: 0,
    last_activity_at: "2026-01-01T00:00:00Z",
    cost_usd: 0,
    input_tokens: 0,
    output_tokens: 0,
    error: null,
    created_at: "2026-01-01T00:00:00Z",
    parked_at: null,
    ended_at: null,
  };
}

/** The refresh only has to resolve; the new token comes from `getAccessToken`. */
const REFRESHED = {} as AuthResponse;

function store() {
  return getSessionStore(SESSION_ID).getState();
}

function last(): FakeSocket {
  const socket = FakeSocket.instances.at(-1);
  if (!socket) throw new Error("no socket was opened");
  return socket;
}

/** Lets the chain of promises inside `start`/`refreshThenReconnect` settle. */
async function settle(): Promise<void> {
  for (let i = 0; i < 5; i += 1) await Promise.resolve();
}

async function startLive(events: AgentEvent[] = []): Promise<SessionSocket> {
  listEvents.mockResolvedValueOnce({ events, has_more: false });
  const socket = new SessionSocket(SESSION_ID, factory);
  await socket.start();
  last().accept();
  return socket;
}

let sockets: SessionSocket[] = [];

beforeEach(() => {
  FakeSocket.instances = [];
  sockets = [];
  disposeSessionStore(SESSION_ID);
  vi.clearAllMocks();
  getAccessToken.mockReturnValue("token-one");
  vi.spyOn(console, "warn").mockImplementation(() => {});
});

afterEach(() => {
  for (const socket of sockets) socket.dispose();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

function track(socket: SessionSocket): SessionSocket {
  sockets.push(socket);
  return socket;
}

describe("SessionSocket", () => {
  it("loads the newest history page, then opens at that cursor", async () => {
    track(await startLive([textEvent(1, "one"), textEvent(2, "two")]));

    expect(listEvents).toHaveBeenCalledWith(SESSION_ID, { limit: 200 });
    expect(last().url).toBe(
      `ws://${globalThis.location.host}/ws/sessions/${SESSION_ID}?after=2&token=token-one`,
    );
    expect(last().binaryType).toBe("arraybuffer");
    expect(store().status).toBe("live");
    expect(store().lastSeq).toBe(2);
  });

  it("skips REST when the store already holds the transcript", async () => {
    getSessionStore(SESSION_ID).getState().applyEvent(textEvent(7, "resumed"));

    const socket = track(new SessionSocket(SESSION_ID, factory));
    await socket.start();

    expect(listEvents).not.toHaveBeenCalled();
    expect(last().url).toContain("after=7");
  });

  it("dedupes an event replayed after a reconnect", async () => {
    track(await startLive([textEvent(1, "one")]));
    const first = last();
    first.text({ type: "event", event: textEvent(2, "two") });
    expect(store().order).toHaveLength(2);

    refreshAccessToken.mockResolvedValue(REFRESHED);
    getAccessToken.mockReturnValue("token-two");
    first.serverClose(1006);
    await settle();

    // The server may resend the tail; only the unseen event lands.
    last().accept();
    last().text({ type: "event", event: textEvent(2, "two") });
    last().text({ type: "event", event: textEvent(3, "three") });

    expect(store().order).toHaveLength(3);
    expect(store().lastSeq).toBe(3);
  });

  it("refreshes the token and reopens at the current cursor", async () => {
    track(await startLive([textEvent(1, "one")]));
    last().text({ type: "event", event: textEvent(4, "four") });

    refreshAccessToken.mockResolvedValue(REFRESHED);
    getAccessToken.mockReturnValue("token-two");
    last().serverClose(1006);
    expect(store().status).toBe("reconnecting");
    await settle();

    expect(refreshAccessToken).toHaveBeenCalledTimes(1);
    expect(FakeSocket.instances).toHaveLength(2);
    expect(last().url).toContain("after=4");
    expect(last().url).toContain("token=token-two");
  });

  it("stops retrying when the refresh answers 401", async () => {
    vi.useFakeTimers();
    track(await startLive());
    refreshAccessToken.mockRejectedValue(
      new ApiError(401, "authentication required"),
    );

    last().serverClose(1006);
    await settle();
    await vi.advanceTimersByTimeAsync(60_000);

    expect(FakeSocket.instances).toHaveLength(1);
  });

  it("stops when the shared refresh completed for a session that is gone", async () => {
    vi.useFakeTimers();
    track(await startLive());
    // `services/auth` rejects this way when the rotation finished after the
    // user signed out: there is nothing left to reconnect to.
    refreshAccessToken.mockRejectedValue(new auth.StaleRefreshError());

    last().serverClose(1006);
    await settle();
    await vi.advanceTimersByTimeAsync(60_000);

    expect(FakeSocket.instances).toHaveLength(1);
  });

  it("backs off after a transient refresh failure", async () => {
    vi.useFakeTimers();
    vi.spyOn(Math, "random").mockReturnValue(0.5); // no jitter
    track(await startLive());
    refreshAccessToken.mockRejectedValue(new ApiError(503, "unavailable"));

    last().serverClose(1006);
    await settle();
    expect(FakeSocket.instances).toHaveLength(1);

    await vi.advanceTimersByTimeAsync(999);
    expect(FakeSocket.instances).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(2);
    expect(FakeSocket.instances).toHaveLength(2);
  });

  it("gives up when a refreshed token is refused again within the window", async () => {
    vi.useFakeTimers();
    track(await startLive());
    refreshAccessToken.mockResolvedValue(REFRESHED);

    last().text({ type: "error", message: "authentication required" });
    last().serverClose(1008);
    await settle();
    expect(FakeSocket.instances).toHaveLength(2);

    last().accept();
    last().serverClose(1008);
    await settle();
    await vi.advanceTimersByTimeAsync(60_000);

    expect(FakeSocket.instances).toHaveLength(2);
    expect(refreshAccessToken).toHaveBeenCalledTimes(1);
  });

  it("shows a non-auth error frame as a system message", async () => {
    track(await startLive());
    last().text({ type: "error", message: "session is gone" });

    const messages = Object.values(store().messages);
    expect(messages).toHaveLength(1);
    expect(messages[0]).toMatchObject({
      kind: "system",
      level: "error",
      text: "session is gone",
    });
    expect(store().lastSeq).toBe(0);
  });

  it("adds the optimistic message before the frame goes out", async () => {
    const socket = track(await startLive());
    const clientId = socket.send({ kind: "message", text: "hello" });

    expect(last().orderAtSend).toEqual([1]);
    expect(last().frames).toEqual([
      JSON.stringify({
        type: "input",
        client_id: clientId,
        input: { kind: "message", text: "hello" },
      }),
    ]);
    expect(store().messages[`client:${clientId}`]).toMatchObject({
      kind: "user",
      text: "hello",
      pending: true,
    });
  });

  it("falls back to REST for input and stop while closed", async () => {
    listEvents.mockResolvedValueOnce({ events: [], has_more: false });
    sendInput.mockResolvedValue(undefined);
    stopSession.mockResolvedValue(undefined);
    const socket = track(new SessionSocket(SESSION_ID, factory));
    await socket.start(); // opened but never accepted: readyState stays 0

    const clientId = socket.send({ kind: "message", text: "hello" });
    socket.stop();

    expect(sendInput).toHaveBeenCalledWith(
      SESSION_ID,
      { kind: "message", text: "hello" },
      clientId,
    );
    expect(stopSession).toHaveBeenCalledWith(SESSION_ID);
    expect(last().frames).toEqual([]);
  });

  it("reconciles the optimistic message of a REST fallback send", async () => {
    listEvents.mockResolvedValueOnce({ events: [], has_more: false });
    sendInput.mockResolvedValue(undefined);
    const socket = track(new SessionSocket(SESSION_ID, factory));
    await socket.start(); // opened but never accepted: readyState stays 0

    const clientId = socket.send({ kind: "message", text: "hello" });
    expect(store().order).toEqual([`client:${clientId}`]);

    // The orchestrator recorded the input and echoed the id back, as it does
    // for the socket path (`SPEC.md`, "Sessions").
    last().accept();
    last().text({
      type: "event",
      event: {
        seq: 1,
        ts: "2026-01-01T00:00:00Z",
        kind: "user_message",
        text: "hello",
        user_id: null,
        client_id: clientId,
      },
    });

    expect(store().order).toEqual(["e1"]);
    expect(store().messages["e1"]).toMatchObject({
      kind: "user",
      text: "hello",
    });
    expect(store().messages["e1"]).not.toHaveProperty("pending", true);
  });

  it("refuses terminal_open unless the session is running", async () => {
    const socket = track(await startLive());
    const frames: unknown[] = [];
    socket.terminal.subscribe((frame) => frames.push(frame));

    socket.terminal.open(80, 24);
    expect(last().frames).toEqual([]);
    expect(frames).toEqual([{ exit_code: -1 }]);

    last().text({ type: "session", session: session("running") });
    socket.terminal.open(80, 24);
    socket.terminal.write(new Uint8Array([104, 105]));
    socket.terminal.close();

    expect(last().frames).toEqual([
      JSON.stringify({ type: "terminal_open", cols: 80, rows: 24 }),
      JSON.stringify({ type: "terminal_close" }),
    ]);
    expect(last().sent[1]).toBeInstanceOf(ArrayBuffer);
  });

  it("delivers binary frames to terminal subscribers", async () => {
    const socket = track(await startLive());
    const frames: unknown[] = [];
    const unsubscribe = socket.terminal.subscribe((frame) => frames.push(frame));

    last().binary(new Uint8Array([1, 2, 3]));
    last().text({ type: "terminal_closed", exit_code: 0 });
    unsubscribe();
    last().binary(new Uint8Array([4]));

    expect(frames).toEqual([new Uint8Array([1, 2, 3]), { exit_code: 0 }]);
  });

  it("pages older history once and stops at seq 1", async () => {
    track(await startLive([textEvent(5, "five")]));
    getSessionStore(SESSION_ID).getState().prependHistory([textEvent(5, "five")], true);

    listEvents.mockResolvedValueOnce({ events: [textEvent(1, "one")], has_more: false });
    const socket = new SessionSocket(SESSION_ID, factory);
    await socket.loadOlder();
    expect(listEvents).toHaveBeenLastCalledWith(SESSION_ID, {
      before: 5,
      limit: 200,
    });
    expect(store().oldestSeq).toBe(1);

    // `hasMore` is false now, and `oldestSeq` is 1 either way: no request.
    listEvents.mockClear();
    await socket.loadOlder();
    expect(listEvents).not.toHaveBeenCalled();
  });

  it("reports a failed older page and loads it on a retry", async () => {
    track(await startLive([textEvent(5, "five")]));
    store().prependHistory([textEvent(5, "five")], true);

    listEvents.mockRejectedValueOnce(new ApiError(503, "network down"));
    const socket = track(new SessionSocket(SESSION_ID, factory));
    expect(await socket.loadOlder()).toBe(false);
    expect(store().historyStatus).toBe("error");
    expect(store().historyError).toBe("network down");
    // The transcript is untouched: the page that failed is the only loss.
    expect(store().order).toHaveLength(1);
    expect(store().oldestSeq).toBe(5);

    // The guard the failed attempt took is gone: the same page is asked for
    // again and lands.
    listEvents.mockResolvedValueOnce({
      events: [textEvent(1, "one")],
      has_more: false,
    });
    expect(await socket.loadOlder()).toBe(true);
    expect(store().historyStatus).toBe("idle");
    expect(store().historyError).toBeNull();
    expect(store().oldestSeq).toBe(1);
    expect(store().order).toHaveLength(2);
  });

  it("coalesces simultaneous older-history requests into one", async () => {
    track(await startLive([textEvent(5, "five")]));
    store().prependHistory([textEvent(5, "five")], true);

    listEvents.mockClear();
    listEvents.mockResolvedValueOnce({
      events: [textEvent(1, "one")],
      has_more: false,
    });
    const socket = track(new SessionSocket(SESSION_ID, factory));
    // A scroll gesture and the transcript's fill effect in the same commit.
    const outcomes = await Promise.all([socket.loadOlder(), socket.loadOlder()]);

    expect(outcomes).toEqual([true, true]);
    expect(listEvents).toHaveBeenCalledTimes(1);
    expect(store().order).toHaveLength(2);
  });

  it("ignores a malformed frame", async () => {
    track(await startLive());
    last().onmessage?.(new MessageEvent("message", { data: "{not json" }));

    expect(store().order).toHaveLength(0);
  });

  it("closes the terminal, then the socket, on dispose", async () => {
    const socket = track(await startLive());
    last().text({ type: "session", session: session("running") });
    socket.terminal.open(80, 24);

    const opened = last();
    socket.dispose();

    expect(opened.frames.at(-1)).toBe(JSON.stringify({ type: "terminal_close" }));
    expect(opened.closed).toEqual([1000]);
  });

  it("reopens with the new token when credentials are replaced", async () => {
    const registered: { replaced: ((token: string) => void) | null } = {
      replaced: null,
    };
    onCredentialsReplaced.mockImplementation((handler) => {
      registered.replaced = handler;
      return () => {};
    });
    track(await startLive([textEvent(9, "nine")]));

    getAccessToken.mockReturnValue("token-two");
    registered.replaced?.("token-two");

    expect(FakeSocket.instances).toHaveLength(2);
    expect(last().url).toContain("token=token-two");
    expect(last().url).toContain("after=9");
    expect(refreshAccessToken).not.toHaveBeenCalled();
  });
});
