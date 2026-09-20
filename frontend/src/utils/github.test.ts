import { describe, expect, it } from "vitest";
import { githubCompareUrl } from "./github";

// Obviously fake remotes (CLAUDE.md, rule 3).
const SESSION_BRANCH = "session/00000000-0000-4000-8000-0000000000a1";

describe("githubCompareUrl", () => {
  it("builds the compare page for an https remote ending in .git", () => {
    expect(
      githubCompareUrl("https://github.com/owner/repo.git", "main", "topic"),
    ).toBe("https://github.com/owner/repo/compare/main...topic?expand=1");
  });

  it("builds it for a remote without the .git suffix", () => {
    expect(
      githubCompareUrl("https://github.com/owner/repo", "main", "topic"),
    ).toBe("https://github.com/owner/repo/compare/main...topic?expand=1");
  });

  it("strips a trailing slash and keeps the case of owner and repo", () => {
    expect(
      githubCompareUrl("https://github.com/Owner/Repo/", "main", "topic"),
    ).toBe("https://github.com/Owner/Repo/compare/main...topic?expand=1");
  });

  it("keeps the slash in a session branch name", () => {
    expect(
      githubCompareUrl(
        "https://github.com/owner/repo.git",
        "main",
        SESSION_BRANCH,
      ),
    ).toBe(
      `https://github.com/owner/repo/compare/main...${SESSION_BRANCH}?expand=1`,
    );
  });

  it("answers null for any other host", () => {
    expect(
      githubCompareUrl("https://gitlab.com/owner/repo", "main", "topic"),
    ).toBeNull();
  });

  it("answers null for an ssh remote and for a path that is not owner/repo", () => {
    expect(
      githubCompareUrl("git@github.com:owner/repo.git", "main", "topic"),
    ).toBeNull();
    expect(
      githubCompareUrl("https://github.com/owner", "main", "topic"),
    ).toBeNull();
    expect(
      githubCompareUrl(
        "https://github.com/owner/repo/tree/main",
        "main",
        "topic",
      ),
    ).toBeNull();
  });

  it("answers null without a target or a remote branch", () => {
    expect(githubCompareUrl("https://github.com/owner/repo", "", "topic")).toBeNull();
    expect(githubCompareUrl("https://github.com/owner/repo", "main", "")).toBeNull();
  });
});
