// What an expiring access token does to an *open* task stream (`SPEC.md`,
// "Authentication": an open stream is not closed because its token expired,
// and only a self-service password change reopens one).
//
// Unlike `useTaskStream.test.ts`, this suite runs against the real
// `services/auth` and `services/apiClient`, so the rotation is a genuine REST
// 401 followed by `POST /auth/refresh` rather than a hand-called handler.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { apiGet } from "../services/apiClient";
import { clearAuth, installSession } from "../services/auth";
import * as tasks from "../services/tasks";
import * as taskStates from "../services/taskStates";
import type { AuthResponse, User } from "../types";
import { useTaskStore } from "./taskStore";
import { TaskStream } from "./useTaskStream";

vi.mock("../services/tasks", async (original) => ({
  ...(await original<typeof tasks>()),
  listTasks: vi.fn(),
}));

vi.mock("../services/taskStates", async (original) => ({
  ...(await original<typeof taskStates>()),
  listTaskStates: vi.fn(),
}));

const listTasks = vi.mocked(tasks.listTasks);
const listTaskStates = vi.mocked(taskStates.listTaskStates);

const PROJECT = "11111111-1111-4111-8111-111111111111";

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

class FakeEventSource {
  static instances: FakeEventSource[] = [];

  readonly url: string;
  closed = 0;
  onopen: ((ev: Event) => void) | null = null;
  onerror: ((ev: Event) => void) | null = null;

  constructor(url: string) {
    this.url = url;
    FakeEventSource.instances.push(this);
  }

  addEventListener(): void {
    // The rotation cases send no events.
  }

  close(): void {
    this.closed += 1;
  }

  accept(): void {
    this.onopen?.(new Event("open"));
  }
}

const factory = (url: string) => new FakeEventSource(url);

function last(): FakeEventSource {
  const source = FakeEventSource.instances.at(-1);
  if (!source) throw new Error("no stream was opened");
  return source;
}

function board() {
  return useTaskStore.getState();
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

/** Lets the promise chain inside the store's refresh settle. */
async function settle(): Promise<void> {
  for (let i = 0; i < 5; i += 1) await Promise.resolve();
}

let streams: TaskStream[] = [];

beforeEach(() => {
  FakeEventSource.instances = [];
  streams = [];
  vi.clearAllMocks();
  listTasks.mockResolvedValue([]);
  listTaskStates.mockResolvedValue([]);
  fetchMock.mockReset();
  globalThis.fetch = fetchMock;
  clearAuth();
  installSession(authResponse("token-one"));
});

afterEach(() => {
  for (const stream of streams) stream.dispose();
  clearAuth();
  vi.restoreAllMocks();
});

async function liveStream(): Promise<TaskStream> {
  const stream = new TaskStream(PROJECT, factory);
  streams.push(stream);
  stream.start();
  last().accept();
  await settle();
  expect(board().stream).toBe("live");
  return stream;
}

describe("TaskStream across a token rotation", () => {
  it("keeps the stream open through a REST 401 refresh", async () => {
    await liveStream();
    const open = last();
    // The TasksPanel poll 15 minutes in: 401, one rotation, retry.
    fetchMock.mockImplementation((input) => {
      if (urlOf(input) === "/api/auth/refresh") {
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
          : fakeResponse(200, "[]"),
      );
    });

    await apiGet(`/projects/${PROJECT}/tasks`);

    expect(FakeEventSource.instances).toHaveLength(1);
    expect(last()).toBe(open);
    expect(open.closed).toBe(0);
    expect(board().stream).toBe("live");
  });

  it("reopens exactly once for a self-service password change", async () => {
    await liveStream();
    const open = last();

    installSession(authResponse("token-three"), "password_change");

    expect(FakeEventSource.instances).toHaveLength(2);
    expect(open.closed).toBe(1);
    expect(last().url).toContain("token=token-three");
    expect(board().stream).toBe("reconnecting");
  });
});
