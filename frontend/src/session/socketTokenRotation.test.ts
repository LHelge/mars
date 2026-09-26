// What an expiring access token does to an *open* session socket
// (`SPEC.md`, "Authentication": "an open stream is not closed merely because
// that token later expires", and its rule that only a self-service password
// change reopens streams).
//
// Unlike `useSessionSocket.test.ts`, this suite runs against the real
// `services/auth` and `services/apiClient`: the thing under test is the seam
// between them and `SessionSocket`, so a rotation here is a genuine REST 401
// followed by `POST /auth/refresh`, not a hand-called handler.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { apiGet } from "../services/apiClient";
import { clearAuth, installSession } from "../services/auth";
import * as sessions from "../services/sessions";
import type { AuthResponse, Session, SessionState, User } from "../types";
import { disposeSessionStore, getSessionStore } from "./sessionStore";
import type { SocketLike } from "./useSessionSocket";
import { SessionSocket } from "./useSessionSocket";

vi.mock("../services/sessions", () => ({
  listEvents: vi.fn(),
  sendInput: vi.fn(),
  stopSession: vi.fn(),
}));

const listEvents = vi.mocked(sessions.listEvents);

const SESSION_ID = "11111111-1111-4111-8111-111111111111";

const user: User = {
  id: "00000000-0000-0000-0000-000000000001",
  username: "tester",
  email: "tester@example.invalid",
  admin: false,
  must_change_password: false,
  notify_email: true,
  created_at: "2026-01-01T00:00:00Z",
};

function authResponse(token: string): AuthResponse {
  return { user, access_token: token };
}

function session(state: SessionState): Session {
  return {
    id: SESSION_ID,
    project_id: "22222222-2222-4222-8222-222222222222",
    profile_id: "33333333-3333-4333-8333-333333333333",
    kind: "conversational",
    created_by: null,
    launch_source: "user",
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

class FakeSocket implements SocketLike {
  static instances: FakeSocket[] = [];

  binaryType: BinaryType = "blob";
  readyState = 0;
  readonly url: string;
  readonly sent: (string | ArrayBuffer)[] = [];
  readonly closed: number[] = [];

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
  }

  close(code = 1000): void {
    this.readyState = 3;
    this.closed.push(code);
  }

  accept(): void {
    this.readyState = 1;
    this.onopen?.(new Event("open"));
  }

  text(message: unknown): void {
    this.onmessage?.(
      new MessageEvent("message", { data: JSON.stringify(message) }),
    );
  }

  get frames(): string[] {
    return this.sent.filter((data): data is string => typeof data === "string");
  }
}

const factory = (url: string): SocketLike => new FakeSocket(url);

function store() {
  return getSessionStore(SESSION_ID).getState();
}

function last(): FakeSocket {
  const socket = FakeSocket.instances.at(-1);
  if (!socket) throw new Error("no socket was opened");
  return socket;
}

function fakeResponse(status: number, body?: string): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    statusText: "Status Text",
    text: () => Promise.resolve(body ?? ""),
  } as unknown as Response;
}

function urlOf(input: RequestInfo | URL): string {
  if (typeof input !== "string") {
    throw new TypeError("apiClient passed a non-string URL");
  }
  return input;
}

const fetchMock = vi.fn<typeof fetch>();

/** An open socket with the terminal attached, as the session view has it. */
async function liveSocketWithTerminal(): Promise<SessionSocket> {
  listEvents.mockResolvedValueOnce({ events: [], has_more: false });
  const socket = new SessionSocket(SESSION_ID, factory);
  await socket.start();
  last().accept();
  last().text({ type: "session", session: session("running") });
  socket.terminal.open(80, 24);
  expect(last().frames).toContain(
    JSON.stringify({ type: "terminal_open", cols: 80, rows: 24 }),
  );
  return socket;
}

let sockets: SessionSocket[] = [];

beforeEach(() => {
  FakeSocket.instances = [];
  sockets = [];
  disposeSessionStore(SESSION_ID);
  vi.clearAllMocks();
  fetchMock.mockReset();
  globalThis.fetch = fetchMock;
  clearAuth();
  installSession(authResponse("token-one"));
});

afterEach(() => {
  for (const socket of sockets) socket.dispose();
  clearAuth();
  vi.restoreAllMocks();
});

function track(socket: SessionSocket): SessionSocket {
  sockets.push(socket);
  return socket;
}

describe("SessionSocket across a token rotation", () => {
  it("keeps the socket and its terminal through a REST 401 refresh", async () => {
    const socket = track(await liveSocketWithTerminal());
    const open = last();
    // The window-focus refetch of the session detail, 15 minutes in: 401,
    // one rotation, retry.
    fetchMock.mockImplementation((input) => {
      const url = urlOf(input);
      if (url === "/api/auth/refresh") {
        return Promise.resolve(
          fakeResponse(200, JSON.stringify(authResponse("token-two"))),
        );
      }
      return Promise.resolve(
        fetchMock.mock.calls.length === 1
          ? fakeResponse(
              401,
              JSON.stringify({ status: 401, error: "authentication required" }),
            )
          : fakeResponse(200, JSON.stringify(session("running"))),
      );
    });

    await apiGet<Session>(`/sessions/${SESSION_ID}`);

    // Nothing was torn down: same socket, never closed, terminal still open.
    expect(FakeSocket.instances).toHaveLength(1);
    expect(last()).toBe(open);
    expect(open.closed).toEqual([]);
    expect(store().status).toBe("live");
    socket.terminal.write(new Uint8Array([0x6c, 0x73]));
    expect(open.sent.at(-1)).toBeInstanceOf(ArrayBuffer);
  });

  it("reopens exactly once for a self-service password change", async () => {
    track(await liveSocketWithTerminal());
    const open = last();

    // What `PasswordChangeForm` does with the pair the change returned.
    installSession(authResponse("token-three"), "password_change");

    expect(FakeSocket.instances).toHaveLength(2);
    expect(open.closed).toEqual([1000]);
    expect(last().url).toContain("token=token-three");
    expect(store().status).toBe("reconnecting");
  });
});
