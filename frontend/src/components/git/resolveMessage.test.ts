import { describe, expect, it } from "vitest";

import type { Branch, Profile } from "../../types";
import {
  commitOfSource,
  conversationalProfiles,
  resolveMessage,
  selectedResolver,
  sourceOfRef,
} from "./resolveMessage";
import type { ResolveMerge } from "./resolveMessage";

const SID = "33333333-3333-4333-8333-333333333333";
const HID = "44444444-4444-4444-8444-444444444444";
const SOURCE_COMMIT = "a".repeat(40);
const TARGET_COMMIT = "b".repeat(40);

function merge(source: ResolveMerge["source"]): ResolveMerge {
  return {
    source,
    sourceCommit: SOURCE_COMMIT,
    target: "main",
    targetCommit: TARGET_COMMIT,
    paths: ["CLAUDE.md", "src/app.txt"],
  };
}

const TAIL =
  "Resolve the conflicts, run the project's checks, commit the merge, and tell me when your branch is ready to merge into main. Do not merge into main yourself.";

describe("resolveMessage", () => {
  it("names a session source by its ref and fetches it by that ref", () => {
    expect(resolveMessage(merge({ kind: "session", sessionId: SID }))).toBe(
      [
        `Merge refs/sessions/${SID} (${SOURCE_COMMIT}) into your branch, which starts at main (bbbbbbb).`,
        "The merge Mars tried conflicted in:",
        "- CLAUDE.md",
        "- src/app.txt",
        `Fetch the source with: git fetch origin refs/sessions/${SID}`,
        TAIL,
      ].join("\n"),
    );
  });

  it("fetches a hand-off by its hand-off ref", () => {
    const text = resolveMessage(merge({ kind: "handoff", handoffId: HID }));
    expect(text).toContain(`Merge refs/handoffs/${HID} (${SOURCE_COMMIT})`);
    expect(text).toContain(
      `Fetch the source with: git fetch origin refs/handoffs/${HID}`,
    );
  });

  it("fetches an integration head by its branch name", () => {
    const text = resolveMessage(merge({ kind: "head", name: "release" }));
    expect(text).toContain(`Merge release (${SOURCE_COMMIT})`);
    expect(text).toContain("Fetch the source with: git fetch origin release\n");
  });

  it("fetches an upstream-tracking ref by its full refspec", () => {
    const text = resolveMessage(
      merge({ kind: "upstream", name: "origin/main" }),
    );
    expect(text).toContain(`Merge origin/main (${SOURCE_COMMIT})`);
    expect(text).toContain(
      "Fetch the source with: git fetch origin refs/remotes/origin/main",
    );
  });

  it("leaves out a commit it cannot name rather than guessing", () => {
    const text = resolveMessage({
      ...merge({ kind: "session", sessionId: SID }),
      sourceCommit: null,
      targetCommit: null,
    });
    expect(text.split("\n")[0]).toBe(
      `Merge refs/sessions/${SID} into your branch, which starts at main.`,
    );
    expect(text).not.toContain("(");
  });
});

const BRANCHES: Branch[] = [
  { name: "main", kind: "head", commit: TARGET_COMMIT },
  { name: "origin/main", kind: "upstream", commit: "c".repeat(40) },
  {
    name: `refs/sessions/${SID}`,
    kind: "session",
    commit: SOURCE_COMMIT,
    session_id: SID,
  },
];

describe("sourceOfRef and commitOfSource", () => {
  it("reads each kind from the listing", () => {
    expect(sourceOfRef("main", BRANCHES)).toEqual({
      kind: "head",
      name: "main",
    });
    expect(sourceOfRef("origin/main", BRANCHES)).toEqual({
      kind: "upstream",
      name: "origin/main",
    });
    expect(sourceOfRef(`refs/sessions/${SID}`, BRANCHES)).toEqual({
      kind: "session",
      sessionId: SID,
    });
  });

  it("reads the kind from the spelling when the listing lacks the ref", () => {
    expect(sourceOfRef("origin/dev", [])).toEqual({
      kind: "upstream",
      name: "origin/dev",
    });
    expect(sourceOfRef(`refs/sessions/${SID}`, [])).toEqual({
      kind: "session",
      sessionId: SID,
    });
  });

  it("finds a source's commit, or none", () => {
    expect(commitOfSource({ kind: "session", sessionId: SID }, BRANCHES)).toBe(
      SOURCE_COMMIT,
    );
    expect(commitOfSource({ kind: "head", name: "main" }, BRANCHES)).toBe(
      TARGET_COMMIT,
    );
    expect(commitOfSource({ kind: "head", name: "gone" }, BRANCHES)).toBeNull();
  });
});

function profile(
  name: string,
  kind: Profile["kind"],
  isDefault = false,
): Profile {
  return {
    id: `id-${name}`,
    project_id: "22222222-2222-4222-8222-222222222222",
    name,
    kind,
    backend: "claude",
    model: null,
    system_prompt: null,
    permission_mode: "bypass",
    image: "example.invalid/mars/session:1",
    runtime: null,
    mcp_tools: [],
    secrets: [],
    serves_states: [],
    partial_messages: kind === "conversational",
    idle_timeout_secs: 1800,
    is_default: isDefault,
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

describe("selectedResolver", () => {
  const claude = profile("claude", "conversational", true);
  const planner = profile("planner", "conversational");
  const resolver = profile("resolver", "conversational");
  const implementer = profile("implementer", "ephemeral");

  it("offers conversational profiles only", () => {
    expect(
      conversationalProfiles([claude, implementer, resolver]).map(
        (p) => p.name,
      ),
    ).toEqual(["claude", "resolver"]);
  });

  it("preselects a profile named resolver", () => {
    expect(selectedResolver([claude, planner, resolver], "")).toBe(resolver);
  });

  it("falls back to the project's default profile", () => {
    expect(selectedResolver([planner, claude], "")).toBe(claude);
  });

  it("falls back to the first offered when the default is not offered", () => {
    expect(
      selectedResolver(conversationalProfiles([implementer, planner]), ""),
    ).toBe(planner);
  });

  it("keeps the user's choice while it is offered", () => {
    expect(selectedResolver([claude, resolver], claude.id)).toBe(claude);
    expect(selectedResolver([claude, resolver], planner.id)).toBe(resolver);
  });

  it("has nothing to select without a conversational profile", () => {
    expect(selectedResolver([], "")).toBeUndefined();
  });
});
