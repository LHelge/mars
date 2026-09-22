// The launch rules of `SPEC.md`, "Frontend", "Task board" (which profile a
// launch starts on) and "Sessions" (the four refusals the API answers 409 for).

import { describe, expect, it } from "vitest";

import type { Profile, ProfileKind, Task } from "../types";
import { defaultProfile, launchDisabledReason } from "./launchRules";

const PROJECT = "11111111-1111-4111-8111-111111111111";

function profile(
  name: string,
  kind: ProfileKind,
  serves: string[] = [],
): Profile {
  return {
    id: `profile-${name}`,
    project_id: PROJECT,
    name,
    kind,
    backend: "claude",
    model: null,
    system_prompt: null,
    permission_mode: "bypass",
    image: "localhost/mars-session:dev",
    runtime: null,
    mcp_tools: [],
    secrets: [],
    serves_states: serves,
    partial_messages: true,
    idle_timeout_secs: 600,
    is_default: false,
    auto_launch: false,
    max_concurrent: 1,
    schedule_cron: null,
    schedule_prompt: null,
    last_scheduled_at: null,
    next_scheduled_at: null,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
  };
}

const PLANNER = profile("planner", "conversational", ["backlog"]);
const CODER = profile("coder", "conversational", ["ready"]);
const REVIEWER = profile("reviewer", "conversational", ["review", "ready"]);
const ONE_SHOT = profile("one-shot", "ephemeral", []);
const TRIAGE = profile("triage", "ephemeral", ["ready"]);

const ALL = [PLANNER, CODER, REVIEWER, ONE_SHOT, TRIAGE];

describe("defaultProfile", () => {
  it("takes the first profile of the kind that serves the task's state", () => {
    expect(defaultProfile(ALL, "conversational", "ready")).toBe(CODER);
    expect(defaultProfile(ALL, "ephemeral", "ready")).toBe(TRIAGE);
  });

  it("falls back to the first of the kind when none serves the state", () => {
    expect(defaultProfile(ALL, "conversational", "needs_human")).toBe(PLANNER);
    expect(defaultProfile(ALL, "ephemeral", "needs_human")).toBe(ONE_SHOT);
  });

  it("never crosses kinds", () => {
    expect(defaultProfile([CODER], "ephemeral", "ready")).toBeUndefined();
    expect(defaultProfile([], "conversational", "ready")).toBeUndefined();
  });
});

function task(
  fields: Partial<Task> = {},
): Pick<Task, "blocked" | "lease_holder_session_id"> {
  return {
    blocked: false,
    lease_holder_session_id: null,
    ...fields,
  };
}

describe("launchDisabledReason", () => {
  it("allows an unheld, unblocked task in a queue state of a ready project", () => {
    expect(launchDisabledReason(task(), "queue", "ready")).toBeNull();
  });

  it("allows a task waiting in a human state", () => {
    expect(launchDisabledReason(task(), "human", "ready")).toBeNull();
  });

  it("refuses a held task first", () => {
    expect(
      launchDisabledReason(
        task({ lease_holder_session_id: "session-1", blocked: true }),
        "terminal",
        "cloning",
      ),
    ).toBe("Held by a session; release it first");
  });

  it("refuses a blocked task", () => {
    expect(
      launchDisabledReason(task({ blocked: true }), "queue", "ready"),
    ).toBe("Blocked by open dependencies or children");
  });

  it("refuses a terminal state", () => {
    expect(launchDisabledReason(task(), "terminal", "ready")).toBe(
      "Closed tasks cannot be launched",
    );
  });

  it("refuses a project that is not ready, including one not yet read", () => {
    expect(launchDisabledReason(task(), "queue", "cloning")).toBe(
      "Project is not ready",
    );
    expect(launchDisabledReason(task(), "queue", "error")).toBe(
      "Project is not ready",
    );
    expect(launchDisabledReason(task(), "queue", undefined)).toBe(
      "Project is not ready",
    );
  });

  it("does not assume an unknown state is terminal", () => {
    expect(launchDisabledReason(task(), undefined, "ready")).toBeNull();
  });
});
