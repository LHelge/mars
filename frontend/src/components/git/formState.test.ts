// `compareUrlFor`, the rule that decides whether a push gets a compare link
// (`SPEC.md`, "Frontend", "Changes panel").
//
// The end-to-end scenario for the link is skipped — the only remote the suite
// can push to is a local `file://` repository, which has no compare page
// (`frontend/tests/git.spec.ts`, `the github compare link is built
// client-side`) — so the whole of this decision is covered here and in
// `src/utils/github.test.ts`, which owns the URL builder itself.

import { describe, expect, it } from "vitest";

import { compareUrlFor } from "./formState";

const REMOTE = "https://github.com/acme/widgets.git";
const SESSION_BRANCH = "session/11111111-2222-3333-4444-555555555555";

describe("compareUrlFor", () => {
  it("compares the remote branch against the target it was pushed for", () => {
    expect(compareUrlFor(REMOTE, "main", SESSION_BRANCH)).toBe(
      `https://github.com/acme/widgets/compare/main...${SESSION_BRANCH}?expand=1`,
    );
  });

  it("offers no link when the push had no target", () => {
    // `target` is null for a push whose form never named one: there is nothing
    // to compare against.
    expect(compareUrlFor(REMOTE, null, SESSION_BRANCH)).toBeNull();
  });

  it("offers no link when the branch was pushed under the target's own name", () => {
    // A head pushed as itself would compare `main...main`, which is an empty
    // page on GitHub.
    expect(compareUrlFor(REMOTE, "main", "main")).toBeNull();
  });

  it("offers no link for a remote that is not on GitHub", () => {
    expect(compareUrlFor("file:///srv/repos/widgets.git", "main", "work")).toBe(
      null,
    );
    expect(
      compareUrlFor("https://gitlab.com/acme/widgets.git", "main", "work"),
    ).toBeNull();
  });
});
