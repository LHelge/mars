// The transcript view, driven by the same hand-written `AgentEvent` fixtures
// the store fold is tested with.
//
// jsdom lays nothing out, so the virtualizer would see a zero-height scroller
// and render one row: `offsetHeight` is stubbed to give the scroll container a
// viewport and every row a height, which is exactly what `@tanstack/virtual`
// reads (`getRect`, `measureElement`).

import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { act } from "react";

import type { AgentEvent } from "../types";
import { Transcript } from "./Transcript";
import { disposeSessionStore, getSessionStore } from "./sessionStore";

import simpleTurnFixture from "./fixtures/simple_turn.json";
import subagentFixture from "./fixtures/subagent.json";

// The fixtures are JSON, so TypeScript widens their literal `kind` to `string`.
function events(fixture: unknown): AgentEvent[] {
  return fixture as AgentEvent[];
}

const SCROLLER_PX = 600;
const ROW_PX = 24;

beforeAll(() => {
  // Store updates made outside `render` are wrapped in `act`, which React only
  // honours once the environment declares itself an act environment.
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
    true;
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", {
    configurable: true,
    get(this: HTMLElement) {
      return this.dataset.testid === "transcript-scroll" ? SCROLLER_PX : ROW_PX;
    },
  });
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", {
    configurable: true,
    get: () => 800,
  });
});

let nextSession = 0;

/** A fresh session id with `list` already folded into its store. */
function seed(list: AgentEvent[]): string {
  const sessionId = `session-${(nextSession += 1)}`;
  const store = getSessionStore(sessionId);
  for (const event of list) {
    store.getState().applyEvent(event);
  }
  return sessionId;
}

const sessions: string[] = [];

function mount(list: AgentEvent[]) {
  const sessionId = seed(list);
  sessions.push(sessionId);
  return { sessionId, ...render(<Transcript sessionId={sessionId} />) };
}

afterEach(() => {
  cleanup();
  for (const sessionId of sessions.splice(0)) {
    disposeSessionStore(sessionId);
  }
});

/** The rendered rows, in the order they appear in the DOM. */
function renderedKinds(): string[] {
  return Array.from(document.querySelectorAll("[data-message-kind]")).map(
    (element) => element.getAttribute("data-message-kind") ?? "",
  );
}

describe("Transcript", () => {
  it("renders the folded messages in order", () => {
    mount(events(simpleTurnFixture));

    expect(renderedKinds()).toEqual([
      "system",
      "user",
      "assistant_text",
      "result",
    ]);
    expect(screen.getByText("Rename the widget")).toBeDefined();
    expect(screen.getByText("Renaming the widget.")).toBeDefined();
  });

  it("shows the empty state when the store has no messages", () => {
    mount([]);

    expect(screen.getByText("Nothing has happened yet")).toBeDefined();
    expect(document.querySelectorAll("[data-message-kind]")).toHaveLength(0);
  });

  it("collapses a finished subagent and nests its messages when expanded", () => {
    mount(events(subagentFixture));

    // One top-level row, the `Task` call; its transcript starts folded away.
    expect(renderedKinds()).toEqual(["tool"]);
    expect(screen.getByText("Survey the routes")).toBeDefined();
    expect(screen.queryByTestId("subagent-children")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: /Survey the routes/ }));

    // The nested rows are resolved from `subagents`, not from `order`.
    const nested = screen.getByTestId("subagent-children");
    expect(
      Array.from(nested.querySelectorAll("[data-message-kind]")).map((element) =>
        element.getAttribute("data-message-kind"),
      ),
    ).toEqual(["assistant_text", "tool"]);
    expect(nested.textContent).toContain("Looking at the router.");
  });

  it("folds a running subagent too, behind a frame that says it is running", () => {
    // The same fixture up to `subagent_start`: the agent is still working.
    mount(events(subagentFixture).slice(0, 4));

    expect(screen.queryByTestId("subagent-children")).toBeNull();
    expect(screen.getByText("running")).toBeDefined();

    fireEvent.click(screen.getByRole("button", { name: /Survey the routes/ }));
    expect(screen.getByTestId("subagent-children").textContent).toContain(
      "Looking at the router.",
    );
  });

  it("marks a streaming assistant message with a cursor", () => {
    const { sessionId } = mount([
      {
        seq: 1,
        ts: "2026-01-02T10:00:00Z",
        kind: "text_delta",
        text: "Thinking out loud",
      } as unknown as AgentEvent,
    ]);

    expect(screen.getByTestId("streaming-cursor")).toBeDefined();

    // The completing `text` reuses the streaming message's id: the row keeps
    // its identity and only loses the cursor.
    const before = document.querySelector("[data-message-kind]");
    act(() => {
      getSessionStore(sessionId).getState().applyEvent({
        seq: 2,
        ts: "2026-01-02T10:00:01Z",
        kind: "text",
        text: "Thinking out loud.",
      } as unknown as AgentEvent);
    });
    expect(screen.queryByTestId("streaming-cursor")).toBeNull();
    expect(document.querySelector("[data-message-kind]")).toBe(before);
  });

  it("renders markdown without executing HTML and opens links safely", () => {
    mount([
      {
        seq: 1,
        ts: "2026-01-02T10:00:00Z",
        kind: "text",
        text: "See [the docs](https://example.invalid/docs) <b>now</b>",
      } as unknown as AgentEvent,
    ]);

    const link = screen.getByRole("link", { name: "the docs" });
    expect(link.getAttribute("target")).toBe("_blank");
    expect(link.getAttribute("rel")).toBe("noopener noreferrer");
    // No `rehype-raw`: the markup is text, not an element.
    expect(document.querySelector("b")).toBeNull();
    expect(screen.getByText(/<b>now<\/b>/)).toBeDefined();
  });

  it("asks for older history once when scrolled to the top", () => {
    const sessionId = seed(events(simpleTurnFixture));
    sessions.push(sessionId);
    // A page behind the oldest event held, as the history endpoint reports it.
    act(() => {
      getSessionStore(sessionId).getState().prependHistory([], true);
    });
    const loadOlder = vi.fn();
    render(<Transcript sessionId={sessionId} loadOlder={loadOlder} />);

    const scroller = screen.getByTestId("transcript-scroll");
    fireEvent.scroll(scroller);
    fireEvent.scroll(scroller);

    expect(loadOlder).toHaveBeenCalledTimes(1);
  });

  it("does not ask for older history when there is none", () => {
    const sessionId = seed(events(simpleTurnFixture));
    sessions.push(sessionId);
    const loadOlder = vi.fn();
    render(<Transcript sessionId={sessionId} loadOlder={loadOlder} />);

    fireEvent.scroll(screen.getByTestId("transcript-scroll"));

    expect(loadOlder).not.toHaveBeenCalled();
  });

  it("offers Resend for a rejected message", () => {
    const sessionId = `session-${(nextSession += 1)}`;
    sessions.push(sessionId);
    const store = getSessionStore(sessionId);
    act(() => {
      store.getState().addOptimisticUser("c-9", { kind: "message", text: "hi" });
      store.getState().inputRejected("c-9", "session is parked");
    });
    const onResend = vi.fn();
    render(<Transcript sessionId={sessionId} onResend={onResend} />);

    expect(screen.getByText(/session is parked/)).toBeDefined();
    fireEvent.click(screen.getByRole("button", { name: "Resend" }));

    expect(onResend).toHaveBeenCalledWith("hi");
  });
});
