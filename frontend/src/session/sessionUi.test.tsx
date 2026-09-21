// What the reader opened survives the row being recycled (`SPEC.md`,
// "Transcript rendering"), and goes when the session's transcript does
// (`SPEC.md`, "Session store lifecycle").
//
// Scrolling a virtualised row out of the overscan window unmounts it and
// scrolling back mounts it again, which is exactly what this suite does to a
// row: render, open, unmount, render again.

import { afterEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import { disposeSessionStore, getSessionStore } from "./sessionStore";
import type { ToolMessage } from "./sessionStore";
import { SessionUiContext } from "./sessionUi";
import { ToolFrame } from "./tools/ToolFrame";

const OUTPUT = Array.from({ length: 60 }, (_, i) => `line ${String(i + 1)}`);

function tool(): ToolMessage {
  return {
    id: "e7",
    kind: "tool",
    tool_use_id: "toolu_fake",
    name: "Bash",
    input: { command: "ls -l" },
    result: OUTPUT.join("\n"),
    running: false,
  };
}

/** Whether the tool row's body is showing. */
function frameOpen(): boolean {
  return (
    screen.getByRole("button", { name: /Bash/ }).getAttribute("aria-expanded") ===
    "true"
  );
}

/** One row of one session's transcript, as `Transcript` mounts it. */
function row(sessionId: string | null) {
  return render(
    <SessionUiContext.Provider value={sessionId}>
      <ToolFrame message={tool()} />
    </SessionUiContext.Provider>,
  );
}

const sessions: string[] = [];
let next = 0;

function newSession(): string {
  const sessionId = `ui-${String((next += 1))}`;
  sessions.push(sessionId);
  // The registry is what announces a clear, so the session has to have a store
  // for the UI state to be cleared with it.
  getSessionStore(sessionId);
  return sessionId;
}

afterEach(() => {
  cleanup();
  for (const sessionId of sessions.splice(0)) disposeSessionStore(sessionId);
});

describe("transcript disclosures", () => {
  it("keeps a row open across the unmount a scroll away costs it", () => {
    const sessionId = newSession();
    const first = row(sessionId);

    fireEvent.click(screen.getByRole("button", { name: /Bash/ }));
    expect(frameOpen()).toBe(true);
    expect(screen.getByText(/line 40/)).toBeDefined();

    // Ten rows away: the virtualizer drops this one.
    first.unmount();
    row(sessionId);

    // ... and back, with no second click.
    expect(frameOpen()).toBe(true);
    expect(screen.getByText(/line 40/)).toBeDefined();
  });

  it("keeps an expanded result expanded too", () => {
    const sessionId = newSession();
    const first = row(sessionId);
    fireEvent.click(screen.getByRole("button", { name: /Bash/ }));
    fireEvent.click(screen.getByRole("button", { name: /Show all 60 lines/ }));
    expect(screen.getByText(/line 60/)).toBeDefined();

    first.unmount();
    row(sessionId);

    expect(screen.getByText(/line 60/)).toBeDefined();
  });

  it("is one session's own", () => {
    const one = newSession();
    const other = newSession();
    const first = row(one);
    fireEvent.click(screen.getByRole("button", { name: /Bash/ }));
    first.unmount();

    row(other);

    expect(frameOpen()).toBe(false);
  });

  it("goes with the session's transcript", () => {
    const sessionId = newSession();
    const first = row(sessionId);
    fireEvent.click(screen.getByRole("button", { name: /Bash/ }));
    first.unmount();

    // A sign-out, an eviction or a deleted session: the registry announces it
    // and nothing of that transcript is kept, this included (ADR 0027).
    disposeSessionStore(sessionId);
    row(sessionId);

    expect(frameOpen()).toBe(false);
  });

  it("stays in the component outside a transcript", () => {
    const first = row(null);
    fireEvent.click(screen.getByRole("button", { name: /Bash/ }));
    expect(frameOpen()).toBe(true);

    first.unmount();
    row(null);

    expect(frameOpen()).toBe(false);
  });
});
