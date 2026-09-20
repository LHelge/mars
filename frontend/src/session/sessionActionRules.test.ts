// The buttons a session offers, per state, held against the contract in
// `SPEC.md`, "Sessions" and the state table of `ARCHITECTURE.md`, "Session
// lifecycle" — including the `creating` session whose End cancels its launch
// (task `qhyhw`).

import { describe, expect, it } from "vitest";

import type { Session, SessionKind, SessionState } from "../types";
import { sessionActions } from "./sessionActionRules";

// Obviously fake fixture values (CLAUDE.md, rule 3).
const SESSION_ID = "00000000-0000-4000-8000-0000000000a1";
const PROJECT_ID = "00000000-0000-4000-8000-0000000000b2";

function session(
  state: SessionState,
  kind: SessionKind = "conversational",
): Session {
  return {
    id: SESSION_ID,
    project_id: PROJECT_ID,
    profile_id: "00000000-0000-4000-8000-0000000000c3",
    kind,
    created_by: null,
    title: "Fix the login redirect",
    task_id: null,
    handoff_id: null,
    state,
    base_ref: "main",
    branch: "sessions/fix-login",
    container_id: null,
    cli_session_id: null,
    last_seq: 0,
    last_activity_at: "2026-03-01T12:00:00Z",
    cost_usd: 0,
    input_tokens: 0,
    output_tokens: 0,
    error: null,
    created_at: "2026-03-01T11:00:00Z",
    parked_at: null,
    ended_at: null,
  };
}

/** The verbs offered, in the order the header renders them. */
function offered(state: SessionState, kind: SessionKind = "conversational") {
  const can = sessionActions(session(state, kind));
  return (["stop", "end", "sync", "retry", "delete"] as const).filter(
    (verb) => can[verb],
  );
}

describe("sessionActions", () => {
  it("offers End alone while the session is still creating", () => {
    // The end is accepted there: it cancels the launch and closes the session
    // `done`, so the user who launched by mistake does not wait for a
    // container (`SPEC.md`, "Sessions").
    expect(offered("creating")).toEqual(["end"]);
  });

  it("offers Stop, End and Sync while the session runs", () => {
    expect(offered("running")).toEqual(["stop", "end", "sync"]);
  });

  it("offers End and Sync for a parked session", () => {
    expect(offered("parked")).toEqual(["end", "sync"]);
  });

  it("offers Sync and Delete once the session is done", () => {
    expect(offered("done")).toEqual(["sync", "delete"]);
  });

  it("offers Retry and Delete for a failed conversational session", () => {
    expect(offered("failed")).toEqual(["retry", "delete"]);
  });

  it("never offers Retry for an ephemeral session", () => {
    // An ephemeral session is not retried; a new one is launched instead
    // (ADR 0003).
    expect(offered("failed", "ephemeral")).toEqual(["delete"]);
  });

  it("never offers Stop or Delete to a session that has not ended", () => {
    for (const state of ["creating", "running", "parked"] as const) {
      expect(sessionActions(session(state)).delete).toBe(false);
    }
    for (const state of ["creating", "parked", "done", "failed"] as const) {
      expect(sessionActions(session(state)).stop).toBe(false);
    }
  });
});
