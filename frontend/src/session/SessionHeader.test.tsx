// Which actions a session offers is the state table of `ARCHITECTURE.md`,
// "Session lifecycle", made visible: a button that would be refused is not
// shown, and an ephemeral session is never offered a retry.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { Session, SessionKind, SessionState } from "../types";
import { SessionHeader } from "./SessionHeader";
import { disposeSessionStore } from "./sessionStore";

// Obviously fake fixture values (CLAUDE.md, rule 3).
const SESSION_ID = "00000000-0000-4000-8000-0000000000a1";
const PROJECT_ID = "00000000-0000-4000-8000-0000000000b2";

function session(
  state: SessionState,
  kind: SessionKind = "conversational",
  overrides: Partial<Session> = {},
): Session {
  return {
    id: SESSION_ID,
    project_id: PROJECT_ID,
    profile_id: "00000000-0000-4000-8000-0000000000c3",
    kind,
    created_by: null,
    launch_source: "user",
    title: "Fix the login redirect",
    task_id: null,
    handoff_id: null,
    state,
    base_ref: "main",
    branch: "sessions/fix-login",
    container_id: "abcdef0123456789abcdef",
    cli_session_id: "cli-0123456789",
    last_seq: 12,
    last_activity_at: "2026-03-01T12:00:00Z",
    cost_usd: 0.1234,
    input_tokens: 1234567,
    output_tokens: 8910,
    error: null,
    created_at: "2026-03-01T11:00:00Z",
    parked_at: null,
    ended_at: null,
    ...overrides,
  };
}

function mount(value: Session) {
  return render(
    <QueryClientProvider client={new QueryClient()}>
      <MemoryRouter>
        <SessionHeader session={value} status="live" onStop={vi.fn()} />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/** The action buttons currently offered, in DOM order. */
function actions(): string[] {
  return screen
    .getAllByRole("button")
    .map((button) => button.textContent ?? "")
    .filter((label) =>
      ["Stop", "End", "Sync", "Retry", "Delete"].includes(label),
    );
}

/** Whether the stubbed screen is at least Tailwind's `sm` wide. */
let wide = true;

beforeEach(() => {
  // The header as it is from `sm` up, unless a scenario says it is a phone.
  wide = true;
  vi.stubGlobal(
    "matchMedia",
    vi.fn((query: string) => ({
      // Only the width is asked about here.
      matches: query.includes("min-width") ? wide : false,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
    })),
  );
});

afterEach(() => {
  cleanup();
  disposeSessionStore(SESSION_ID);
  vi.unstubAllGlobals();
});

describe("SessionHeader", () => {
  it("offers Stop, End and Sync while the session runs", () => {
    mount(session("running"));
    expect(actions()).toEqual(["Stop", "End", "Sync"]);
  });

  it("offers Retry and Delete for a failed conversational session", () => {
    mount(session("failed", "conversational", { error: "container exited 1" }));
    expect(actions()).toEqual(["Retry", "Delete"]);
    expect(screen.getByRole("alert").textContent).toContain(
      "container exited 1",
    );
  });

  it("never offers Retry for an ephemeral session", () => {
    mount(session("failed", "ephemeral"));
    expect(actions()).toEqual(["Delete"]);
    // ...and says what to do instead (`ARCHITECTURE.md`, "Session lifecycle").
    expect(
      screen
        .getByRole("link", { name: "launch a new one" })
        .getAttribute("href"),
    ).toContain("?tab=sessions");
  });

  it("offers Sync and Delete once the session is done", () => {
    mount(session("done"));
    expect(actions()).toEqual(["Sync", "Delete"]);
  });

  it("offers End alone while the session is still being created", () => {
    mount(session("creating"));
    expect(actions()).toEqual(["End"]);
  });

  it("says a parked session is waiting", () => {
    mount(session("parked"));
    expect(actions()).toEqual(["End", "Sync"]);
    expect(screen.getByText("waiting")).toBeTruthy();
  });

  it("confirms a delete before sending it", () => {
    mount(session("done"));
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(
      screen.getByRole("button", { name: "Delete the session" }),
    ).toBeTruthy();
  });

  it("shows the metadata the operator reads the session by", () => {
    mount(session("running"));

    expect(screen.getByText("sessions/fix-login")).toBeTruthy();
    expect(screen.getByText("main")).toBeTruthy();
    // The container id is shortened to twelve characters, in full on hover.
    const container = screen.getByText("abcdef012345");
    expect(container.getAttribute("title")).toBe("abcdef0123456789abcdef");
    expect(screen.getByText("$0.1234")).toBeTruthy();
    expect(
      screen.getByText((text) => text.replace(/\D/g, "") === "1234567"),
    ).toBeTruthy();
  });

  it("falls back to untitled and opens an editor on click", () => {
    mount(session("running", "conversational", { title: null }));

    expect(screen.getByText("untitled")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Edit title" }));
    expect(screen.getByLabelText("Session title")).toBeTruthy();
  });
});

describe("SessionHeader below sm", () => {
  beforeEach(() => {
    wide = false;
  });

  function details(): HTMLElement {
    return screen.getByRole("button", { name: "Details" });
  }

  it("folds everything but the state, the connection and the title", () => {
    mount(session("running"));

    expect(
      screen.getByRole("status", { name: "Connection live" }),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Edit title" })).toBeTruthy();
    expect(details().getAttribute("aria-expanded")).toBe("false");
    // Closed by default: the actions and the links are not on screen.
    expect(actions()).toEqual([]);
    expect(screen.queryByRole("link", { name: "project" })).toBeNull();
    const body = document.getElementById(
      details().getAttribute("aria-controls") ?? "",
    );
    expect(body?.hidden).toBe(true);
    expect(body?.textContent).toContain("sessions/fix-login");

    fireEvent.click(details());

    expect(details().getAttribute("aria-expanded")).toBe("true");
    expect(body?.hidden).toBe(false);
    expect(actions()).toEqual(["Stop", "End", "Sync"]);
    expect(screen.getByRole("link", { name: "project" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Panels" })).toBeTruthy();
  });

  it("writes the ids out whole, since a phone has no hover", () => {
    mount(session("running"));
    fireEvent.click(details());

    expect(screen.getByText("abcdef0123456789abcdef")).toBeTruthy();
    expect(screen.getByText("cli-0123456789")).toBeTruthy();
  });

  it("keeps a failure in view with Details closed", () => {
    mount(session("failed", "conversational", { error: "container exited 1" }));

    expect(screen.getByRole("alert").textContent).toContain(
      "container exited 1",
    );
  });

  it("opens the branch section as a sheet and gives the focus back", () => {
    mount(session("running"));
    fireEvent.click(details());
    const branch = screen.getByRole("button", { name: "branch" });

    fireEvent.click(branch);

    const sheet = screen.getByRole("dialog", { name: "Session branch" });
    const close = screen.getByRole("button", { name: "Close branch" });
    expect(sheet.contains(close)).toBe(true);
    expect(document.activeElement).toBe(close);
    expect(branch.getAttribute("aria-expanded")).toBe("true");

    fireEvent.keyDown(sheet, { key: "Escape" });

    expect(screen.queryByRole("dialog", { name: "Session branch" })).toBeNull();
    expect(document.activeElement).toBe(branch);
  });
});

describe("SessionTitle", () => {
  it("cancels an edit with the button beside the field", () => {
    const fetchSpy = vi.fn();
    vi.stubGlobal("fetch", fetchSpy);
    mount(session("running"));

    fireEvent.click(screen.getByRole("button", { name: "Edit title" }));
    fireEvent.change(screen.getByLabelText("Session title"), {
      target: { value: "Something else" },
    });
    const cancel = screen.getByRole("button", { name: "Cancel rename" });
    fireEvent.mouseDown(cancel);
    fireEvent.click(cancel);

    expect(screen.queryByLabelText("Session title")).toBeNull();
    expect(screen.getByRole("button", { name: "Edit title" }).textContent).toBe(
      "Fix the login redirect",
    );
    expect(fetchSpy).not.toHaveBeenCalled();
  });
});
