import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { Session, SessionInput, SessionKind, SessionState } from "../types";
import { Composer } from "./Composer";
import { SessionSocketContext } from "./SessionSocketContext";
import {
  disposeSessionStore,
  getSessionStore,
  optimisticId,
} from "./sessionStore";
import { requestResend } from "./sessionUi";
import type { SessionSocketApi } from "./useSessionSocket";

const SESSION_ID = "11111111-1111-4111-8111-111111111111";

function session(
  state: SessionState,
  kind: SessionKind = "conversational",
): Session {
  return {
    id: SESSION_ID,
    project_id: "22222222-2222-4222-8222-222222222222",
    profile_id: "33333333-3333-4333-8333-333333333333",
    kind,
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

function store() {
  return getSessionStore(SESSION_ID).getState();
}

interface FakeSocket {
  api: SessionSocketApi;
  send: ReturnType<typeof vi.fn<(input: SessionInput) => string>>;
  stop: ReturnType<typeof vi.fn<() => void>>;
}

/**
 * The composer's view of the socket. `send` keeps the real hook's contract —
 * it adds the optimistic message and returns its `client_id` — so the
 * rejection path can be driven exactly as the server drives it.
 */
function fakeSocket(status: SessionSocketApi["status"] = "live"): FakeSocket {
  let sent = 0;
  const send = vi.fn((input: SessionInput) => {
    sent += 1;
    const clientId = `client-${String(sent)}`;
    act(() => {
      store().addOptimisticUser(clientId, input);
    });
    return clientId;
  });
  const stop = vi.fn();
  return {
    api: {
      status,
      error: null,
      send,
      stop,
      reconnect: vi.fn(),
      loadOlder: vi.fn(() => Promise.resolve(true)),
      terminal: {
        open: vi.fn(),
        resize: vi.fn(),
        write: vi.fn(),
        close: vi.fn(),
        subscribe: vi.fn(() => () => {}),
      },
    },
    send,
    stop,
  };
}

/** The composer reads its socket from the page's context, as it is mounted. */
function mount(socket: FakeSocket) {
  render(
    <SessionSocketContext.Provider value={socket.api}>
      <Composer sessionId={SESSION_ID} />
    </SessionSocketContext.Provider>,
  );
}

function area(): HTMLTextAreaElement {
  return screen.getByLabelText<HTMLTextAreaElement>("Message");
}

function button(name: string | RegExp): HTMLButtonElement {
  return screen.getByRole<HTMLButtonElement>("button", { name });
}

function type(value: string): void {
  fireEvent.change(area(), { target: { value } });
}

beforeEach(() => {
  disposeSessionStore(SESSION_ID);
});

afterEach(() => {
  cleanup();
  disposeSessionStore(SESSION_ID);
  vi.useRealTimers();
});

describe("Composer visibility", () => {
  it("renders nothing for an ephemeral session", () => {
    act(() => {
      store().setSession(session("running", "ephemeral"));
    });
    mount(fakeSocket());

    expect(screen.queryByLabelText("Message")).toBeNull();
  });

  it("renders nothing before the session is known", () => {
    mount(fakeSocket());

    expect(screen.queryByLabelText("Message")).toBeNull();
  });

  it("disables the box and explains it when the session is done", () => {
    act(() => {
      store().setSession(session("done"));
    });
    mount(fakeSocket());

    expect(area().disabled).toBe(true);
    expect(button("Send").disabled).toBe(true);
    expect(screen.getByText("Session has ended")).toBeTruthy();
  });

  it("points a failed session at Retry", () => {
    act(() => {
      store().setSession(session("failed"));
    });
    mount(fakeSocket());

    expect(area().disabled).toBe(true);
    expect(
      screen.getByText("Session failed — use Retry to relaunch"),
    ).toBeTruthy();
  });

  it("explains what sending does while parked and while creating", () => {
    act(() => {
      store().setSession(session("parked"));
    });
    mount(fakeSocket());
    expect(screen.getByText("Sending will relaunch the session")).toBeTruthy();

    act(() => {
      store().setSession(session("creating"));
    });
    expect(screen.getByText("Queued until the session starts")).toBeTruthy();
  });
});

describe("Composer sending", () => {
  beforeEach(() => {
    act(() => {
      store().setSession(session("running"));
    });
  });

  it("sends a trimmed message on Enter and clears the box", () => {
    const socket = fakeSocket();
    mount(socket);

    type("  hello  ");
    fireEvent.keyDown(area(), { key: "Enter" });

    expect(socket.send).toHaveBeenCalledWith({ kind: "message", text: "hello" });
    expect(area().value).toBe("");
    expect(document.activeElement).toBe(area());
  });

  it("does not send on Shift+Enter", () => {
    const socket = fakeSocket();
    mount(socket);

    type("first line");
    fireEvent.keyDown(area(), { key: "Enter", shiftKey: true });

    expect(socket.send).not.toHaveBeenCalled();
    expect(area().value).toBe("first line");
  });

  it("sends on Cmd/Ctrl+Enter", () => {
    const socket = fakeSocket();
    mount(socket);

    type("ship it");
    fireEvent.keyDown(area(), { key: "Enter", ctrlKey: true });

    expect(socket.send).toHaveBeenCalledWith({
      kind: "message",
      text: "ship it",
    });
  });

  it("does not send while an IME composition is ending", () => {
    const socket = fakeSocket();
    mount(socket);

    type("にほん");
    fireEvent.keyDown(area(), { key: "Enter", isComposing: true });

    expect(socket.send).not.toHaveBeenCalled();
    expect(area().value).toBe("にほん");
  });

  it("refuses blank text", () => {
    const socket = fakeSocket();
    mount(socket);

    type("   ");
    fireEvent.keyDown(area(), { key: "Enter" });

    expect(socket.send).not.toHaveBeenCalled();
    expect(button("Send").disabled).toBe(true);
  });

  it("sends from the button", () => {
    const socket = fakeSocket();
    mount(socket);

    type("via the button");
    fireEvent.click(button("Send"));

    expect(socket.send).toHaveBeenCalledWith({
      kind: "message",
      text: "via the button",
    });
  });

  it("warns about a very large message but still sends it", () => {
    const socket = fakeSocket();
    mount(socket);

    const huge = "x".repeat(100 * 1024 + 1);
    type(huge);
    expect(
      screen.getByText("That is a lot of text. It will be sent as one message."),
    ).toBeTruthy();

    fireEvent.keyDown(area(), { key: "Enter" });
    expect(socket.send).toHaveBeenCalledWith({ kind: "message", text: huge });
  });
});

describe("Composer button label", () => {
  it("reads Interject while a turn is in progress", () => {
    act(() => {
      store().setSession(session("running"));
    });
    mount(fakeSocket());
    expect(button("Send")).toBeTruthy();

    act(() => {
      store().addOptimisticUser("client-x", { kind: "message", text: "go" });
    });
    expect(button("Interject")).toBeTruthy();
  });

  it("stays Send when a parked session still carries a turn flag", () => {
    act(() => {
      store().setSession(session("parked"));
      store().addOptimisticUser("client-x", { kind: "message", text: "go" });
    });
    mount(fakeSocket());

    expect(button("Send")).toBeTruthy();
  });
});

describe("Composer stop button", () => {
  it("is visible only while the session runs", () => {
    act(() => {
      store().setSession(session("parked"));
    });
    mount(fakeSocket());
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();

    act(() => {
      store().setSession(session("running"));
    });
    expect(button("Stop")).toBeTruthy();
  });

  it("shows Stopping… until the session parks", () => {
    const socket = fakeSocket();
    act(() => {
      store().setSession(session("running"));
    });
    mount(socket);

    fireEvent.click(button("Stop"));
    expect(socket.stop).toHaveBeenCalled();
    expect(button("Stopping…").disabled).toBe(true);

    act(() => {
      store().setSession(session("parked"));
    });
    expect(screen.queryByRole("button", { name: /Stop/ })).toBeNull();
  });

  it("re-enables itself after 30 s without a state change", () => {
    vi.useFakeTimers();
    const socket = fakeSocket();
    act(() => {
      store().setSession(session("running"));
    });
    mount(socket);

    fireEvent.click(button("Stop"));
    expect(button("Stopping…").disabled).toBe(true);

    act(() => {
      vi.advanceTimersByTime(30_000);
    });
    expect(button("Stop").disabled).toBe(false);
  });
});

describe("Composer rejection", () => {
  beforeEach(() => {
    act(() => {
      store().setSession(session("running"));
    });
  });

  it("explains a rejection and restores the text", () => {
    const socket = fakeSocket();
    mount(socket);

    type("relaunch please");
    fireEvent.keyDown(area(), { key: "Enter" });
    expect(area().value).toBe("");

    act(() => {
      store().inputRejected("client-1", "session is ephemeral");
    });

    expect(screen.getByRole("alert").textContent).toContain(
      "session is ephemeral",
    );
    expect(area().value).toBe("relaunch please");
    // The restored text is the rejected optimistic message's own text.
    expect(store().messages[optimisticId("client-1")]).toMatchObject({
      kind: "user",
      text: "relaunch please",
      delivery: { state: "rejected", reason: "session is ephemeral" },
    });
  });

  it("keeps what the user has typed since the rejection", () => {
    const socket = fakeSocket();
    mount(socket);

    type("first attempt");
    fireEvent.keyDown(area(), { key: "Enter" });
    type("something else");

    act(() => {
      store().inputRejected("client-1", "not accepted");
    });

    expect(area().value).toBe("something else");
  });

  it("clears the explanation on the next send", () => {
    const socket = fakeSocket();
    mount(socket);

    type("first attempt");
    fireEvent.keyDown(area(), { key: "Enter" });
    act(() => {
      store().inputRejected("client-1", "not accepted");
    });
    expect(screen.queryByRole("alert")).not.toBeNull();

    fireEvent.keyDown(area(), { key: "Enter" });

    expect(socket.send).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole("alert")).toBeNull();
  });
});

describe("Composer extras", () => {
  it("takes the text of a transcript resend and focuses the box", () => {
    act(() => {
      store().setSession(session("running"));
    });
    mount(fakeSocket());

    act(() => {
      requestResend(SESSION_ID, "resent text");
    });

    expect(area().value).toBe("resent text");
    expect(document.activeElement).toBe(area());
  });

  it("takes a resend made before it was mounted", () => {
    act(() => {
      store().setSession(session("running"));
      requestResend(SESSION_ID, "asked for elsewhere");
    });
    mount(fakeSocket());

    expect(area().value).toBe("asked for elsewhere");
  });

  it("does not submit the Enter that confirms an IME candidate on Safari", () => {
    const socket = fakeSocket();
    act(() => {
      store().setSession(session("running"));
    });
    mount(socket);

    type("にほん");
    // `compositionend` has already fired, so `isComposing` is false: the
    // legacy key code is all that is left of the composition.
    fireEvent.keyDown(area(), { key: "Enter", keyCode: 229 });

    expect(socket.send).not.toHaveBeenCalled();
    expect(area().value).toBe("にほん");
  });

  it("says when input is going over HTTP instead of the socket", () => {
    act(() => {
      store().setSession(session("running"));
    });
    mount(fakeSocket("reconnecting"));

    expect(screen.getByText(/Offline, sending over HTTP/)).toBeTruthy();
  });
});
