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
import { peekSessionUi } from "./sessionUi";

import simpleTurnFixture from "./fixtures/simple_turn.json";
import subagentFixture from "./fixtures/subagent.json";

// The fixtures are JSON, so TypeScript widens their literal `kind` to `string`.
function events(fixture: unknown): AgentEvent[] {
  return fixture as AgentEvent[];
}

/** One completed assistant message, for the history paging scenarios. */
function textEvent(seq: number, text: string): AgentEvent {
  return {
    seq,
    ts: "2026-01-02T10:00:00Z",
    kind: "text",
    text,
  } as unknown as AgentEvent;
}

const SCROLLER_PX = 600;
const ROW_PX = 24;

// jsdom lays nothing out, so the scroller's own box is stubbed too: the fill
// effect asks for another page exactly when the content does not overflow the
// viewport, which is a comparison of these two numbers.
let scrollerContentPx = 5000;

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
  Object.defineProperty(HTMLElement.prototype, "clientHeight", {
    configurable: true,
    get(this: HTMLElement) {
      return this.dataset.testid === "transcript-scroll" ? SCROLLER_PX : ROW_PX;
    },
  });
  Object.defineProperty(HTMLElement.prototype, "scrollHeight", {
    configurable: true,
    get(this: HTMLElement) {
      return this.dataset.testid === "transcript-scroll"
        ? scrollerContentPx
        : ROW_PX;
    },
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
  scrollerContentPx = 5000;
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

  it("renders a subagent's report as markdown, and only once the row is opened", () => {
    // The same fixture with a table in the final `tool_result`: the report the
    // subagent handed back.
    const list = events(subagentFixture).map((event) =>
      event.kind === "tool_result" && event.tool_use_id === "toolu_task_fake"
        ? {
            ...event,
            content: "| route | file |\n| --- | --- |\n| `/` | App.tsx |\n",
          }
        : event,
    );
    mount(list);

    // Collapsed: nothing of the report is in the DOM, so nothing is parsed.
    expect(document.querySelector("table")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Task" }));

    const table = document.querySelector("table");
    expect(table).not.toBeNull();
    expect(table?.querySelectorAll("td")).toHaveLength(2);
    // Not the monospace result body: the collapse never sees it.
    expect(screen.queryByText("result")).toBeNull();
  });

  it("shows a user message literally, markdown and all", () => {
    mount([
      {
        seq: 1,
        ts: "2026-01-02T10:00:00Z",
        kind: "user_message",
        text: "*not emphasis* and\n```\nfenced_not_code\n```",
      } as unknown as AgentEvent,
    ]);

    expect(screen.getByText(/\*not emphasis\* and/)).toBeDefined();
    expect(document.querySelector("em")).toBeNull();
    expect(document.querySelector("pre")).toBeNull();
    expect(document.querySelector("code")).toBeNull();
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

    // The reader takes the view to the top; the auto-follow left it at the
    // bottom of the stubbed content.
    const scroller = screen.getByTestId("transcript-scroll");
    scroller.scrollTop = 0;
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

  it("shows the loading indicator only while a request is running", () => {
    const sessionId = seed([textEvent(10, "The latest message")]);
    sessions.push(sessionId);
    const store = getSessionStore(sessionId);
    act(() => {
      store.getState().prependHistory([], true);
    });
    render(<Transcript sessionId={sessionId} loadOlder={vi.fn()} />);

    // Older history remains, but nothing has been asked for yet.
    expect(screen.queryByText("Loading earlier messages")).toBeNull();

    act(() => {
      store.getState().setHistoryStatus("loading");
    });
    expect(screen.getByText("Loading earlier messages")).toBeDefined();

    act(() => {
      store.getState().setHistoryStatus("idle");
    });
    expect(screen.queryByText("Loading earlier messages")).toBeNull();
  });

  it("retries a failed page on demand and keeps the transcript meanwhile", () => {
    const sessionId = seed([textEvent(10, "The latest message")]);
    sessions.push(sessionId);
    const store = getSessionStore(sessionId);
    act(() => {
      store.getState().prependHistory([], true);
    });
    // Stands in for the socket: a request moves the status, and only the
    // status decides what the transcript may ask for next.
    const loadOlder = vi.fn(() => {
      store.getState().setHistoryStatus("loading");
    });
    render(<Transcript sessionId={sessionId} loadOlder={loadOlder} />);

    const scroller = screen.getByTestId("transcript-scroll");
    scroller.scrollTop = 0;
    fireEvent.scroll(scroller);
    expect(loadOlder).toHaveBeenCalledTimes(1);

    act(() => {
      store.getState().setHistoryStatus("error", "network down");
    });

    // The page that failed is still on screen, the spinner is not, and
    // scrolling does not retry into the same failure by itself.
    expect(screen.getByText("The latest message")).toBeDefined();
    expect(screen.queryByText("Loading earlier messages")).toBeNull();
    expect(screen.getByText(/network down/)).toBeDefined();
    fireEvent.scroll(scroller);
    expect(loadOlder).toHaveBeenCalledTimes(1);

    // Retry releases the duplicate-request guard the failed attempt took.
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(loadOlder).toHaveBeenCalledTimes(2);
    expect(screen.getByText("Loading earlier messages")).toBeDefined();

    act(() => {
      store.getState().prependHistory([textEvent(5, "An earlier message")], false);
      store.getState().setHistoryStatus("idle");
    });
    expect(screen.queryByText(/Could not load/)).toBeNull();
    expect(screen.getByText("An earlier message")).toBeDefined();
    expect(screen.getByText("The latest message")).toBeDefined();
  });

  it("keeps the reader on the same row when an older page lands", () => {
    const sessionId = seed([textEvent(10, "The latest message")]);
    sessions.push(sessionId);
    const store = getSessionStore(sessionId);
    act(() => {
      store.getState().prependHistory([], true);
    });
    const loadOlder = vi.fn(() => {
      store.getState().setHistoryStatus("loading");
    });
    render(<Transcript sessionId={sessionId} loadOlder={loadOlder} />);

    const scroller = screen.getByTestId("transcript-scroll");
    scroller.scrollTop = 120;
    fireEvent.scroll(scroller);
    expect(loadOlder).toHaveBeenCalledTimes(1);

    act(() => {
      // The page that arrived is a thousand pixels of rows above the reader.
      scrollerContentPx = 6000;
      store.getState().prependHistory([textEvent(5, "An earlier message")], false);
      store.getState().setHistoryStatus("idle");
    });

    // Without the anchor the reader would be dragged back by the whole page.
    expect(scroller.scrollTop).toBe(1120);
  });

  it("asks for the next page when a landed one does not fill the viewport", () => {
    // 200 tiny `text_delta` events fold into a row or two: nothing overflows,
    // so no `scroll` event will ever be fired for this transcript.
    scrollerContentPx = 100;
    const sessionId = seed([textEvent(10, "The latest message")]);
    sessions.push(sessionId);
    const store = getSessionStore(sessionId);
    act(() => {
      store.getState().prependHistory([], true);
    });
    const loadOlder = vi.fn(() => {
      store.getState().setHistoryStatus("loading");
    });
    render(<Transcript sessionId={sessionId} loadOlder={loadOlder} />);

    // One page, without a gesture.
    expect(loadOlder).toHaveBeenCalledTimes(1);

    act(() => {
      store.getState().prependHistory([textEvent(5, "An earlier message")], true);
      store.getState().setHistoryStatus("idle");
    });
    // The cursor moved and the content still does not overflow: the next page,
    // and only the next one.
    expect(loadOlder).toHaveBeenCalledTimes(2);

    act(() => {
      store.getState().prependHistory([textEvent(1, "The first message")], false);
      store.getState().setHistoryStatus("idle");
    });
    expect(loadOlder).toHaveBeenCalledTimes(2);
  });

  it("offers Resend for a rejected message", () => {
    const sessionId = `session-${(nextSession += 1)}`;
    sessions.push(sessionId);
    const store = getSessionStore(sessionId);
    act(() => {
      store.getState().addOptimisticUser("c-9", { kind: "message", text: "hi" });
      store.getState().inputRejected("c-9", "session is parked");
    });
    render(<Transcript sessionId={sessionId} />);

    expect(screen.getByText(/session is parked/)).toBeDefined();
    fireEvent.click(screen.getByRole("button", { name: "Resend" }));

    // The composer is the one that takes it; what the row does is record it
    // for this session (`sessionUi`).
    expect(peekSessionUi(sessionId)?.resend?.text).toBe("hi");
  });
});
