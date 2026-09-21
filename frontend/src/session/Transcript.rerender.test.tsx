// What a `text_delta` costs the transcript: one row, not the viewport
// (`SPEC.md`, "Frontend", "Transcript rendering").
//
// `Transcript` follows the tail, so it subscribes to the growing message's
// length and re-renders on every delta. The rows it builds are memoised, and
// each subscribes to its own message, so only the row whose message changed
// renders again. This suite counts those renders: the body renderer is
// replaced by one that records the id it was asked for, which is the only way
// to see a render that produces the same DOM as the one before it.
//
// It has a file to itself because that replacement is module-wide, and the
// scenarios of `Transcript.test.tsx` need the real renderer.

import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { cleanup, render } from "@testing-library/react";
import { act } from "react";

import type { AgentEvent } from "../types";
import { Transcript } from "./Transcript";
import { disposeSessionStore, getSessionStore } from "./sessionStore";

const { renders } = vi.hoisted(() => ({ renders: [] as string[] }));

vi.mock("./messages/AssistantText", () => ({
  AssistantText: ({ message }: { message: { id: string; text: string } }) => {
    renders.push(message.id);
    return <div>{message.text}</div>;
  },
}));

const SCROLLER_PX = 600;
const ROW_PX = 24;

// jsdom lays nothing out; the virtualizer reads these three, and with them
// every seeded row is inside the viewport and really rendered.
beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
    true;
  const box = function (this: HTMLElement) {
    return this.dataset.testid === "transcript-scroll" ? SCROLLER_PX : ROW_PX;
  };
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", {
    configurable: true,
    get: box,
  });
  Object.defineProperty(HTMLElement.prototype, "clientHeight", {
    configurable: true,
    get: box,
  });
  Object.defineProperty(HTMLElement.prototype, "scrollHeight", {
    configurable: true,
    get: box,
  });
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", {
    configurable: true,
    get: () => 800,
  });
});

function textEvent(seq: number, text: string): AgentEvent {
  return { seq, ts: "2026-01-02T10:00:00Z", kind: "text", text } as unknown as AgentEvent;
}

function deltaEvent(seq: number, text: string): AgentEvent {
  return {
    seq,
    ts: "2026-01-02T10:00:00Z",
    kind: "text_delta",
    text,
  } as unknown as AgentEvent;
}

const sessions: string[] = [];
let nextSession = 0;

afterEach(() => {
  renders.length = 0;
  cleanup();
  for (const sessionId of sessions.splice(0)) {
    disposeSessionStore(sessionId);
  }
});

describe("Transcript re-rendering", () => {
  it("re-renders only the row a delta grew", () => {
    const sessionId = `rerender-${(nextSession += 1)}`;
    sessions.push(sessionId);
    const store = getSessionStore(sessionId);
    // Five finished messages and one still streaming: the deltas that follow
    // belong to the last one, which is the tail the view is following.
    for (let seq = 1; seq <= 5; seq += 1) {
      store.getState().applyEvent(textEvent(seq, `Message ${seq}`));
    }
    store.getState().applyEvent(deltaEvent(6, "Still "));

    render(<Transcript sessionId={sessionId} />);
    const streamingId = store.getState().order.at(-1);

    // The mount renders every row once; six rows of 24 px fit the viewport.
    expect(renders).toHaveLength(6);
    renders.length = 0;

    act(() => {
      store.getState().applyEvent(deltaEvent(7, "writing"));
    });

    expect(renders).toEqual([streamingId]);

    act(() => {
      store.getState().applyEvent(deltaEvent(8, " this."));
    });

    // And it stays one row per delta: nothing accumulates work per event.
    expect(renders).toEqual([streamingId, streamingId]);
  });
});
