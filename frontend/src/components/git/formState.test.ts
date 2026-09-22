// `useGitAction`, the one owner of a git form's three answers, and
// `compareUrlFor`, the rule that decides whether a push gets a compare link
// (`SPEC.md`, "Frontend", "Changes panel").
//
// The conflict half matters because every git form here handles a 422 rather
// than failing on it: the paths are a list to read, and a refusal without
// paths is still an error banner.
//
// The end-to-end scenario for the link is skipped — the only remote the suite
// can push to is a local `file://` repository, which has no compare page
// (`frontend/tests/git.spec.ts`, `the github compare link is built
// client-side`) — so the whole of this decision is covered here and in
// `src/utils/github.test.ts`, which owns the URL builder itself.

import { act, renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { ApiError } from "../../services/apiClient";
import { compareUrlFor, useGitAction } from "./formState";

const REMOTE = "https://github.com/acme/widgets.git";
const SESSION_BRANCH = "session/11111111-2222-3333-4444-555555555555";

describe("useGitAction", () => {
  it("keeps what the action produced", async () => {
    const { result } = renderHook(() =>
      useGitAction(() => Promise.resolve("a")),
    );

    act(() => {
      result.current.submit();
    });

    await waitFor(() => {
      expect(result.current.result).toBe("a");
    });
    expect(result.current.conflict).toBeNull();
    expect(result.current.error).toBeNull();
  });

  it("reads the paths and the sentence off a 422", async () => {
    const conflict = new ApiError(422, "merge conflict", [
      "src/one.ts",
      "src/two.ts",
    ]);
    const { result } = renderHook(() =>
      useGitAction(() => Promise.reject(conflict)),
    );

    act(() => {
      result.current.submit();
    });

    await waitFor(() => {
      expect(result.current.conflict).toEqual({
        paths: ["src/one.ts", "src/two.ts"],
        message: "merge conflict",
      });
    });
    // A conflict is an outcome, not a failure: no alert beside the list.
    expect(result.current.error).toBeNull();
    expect(result.current.result).toBeNull();
  });

  it("fails on a refusal that carries no paths, whatever the status", async () => {
    const { result } = renderHook(() =>
      useGitAction(() => Promise.reject(new ApiError(422, "merge conflict"))),
    );

    act(() => {
      result.current.submit();
    });

    await waitFor(() => {
      expect(result.current.error).toBe("merge conflict");
    });
    expect(result.current.conflict).toBeNull();
  });

  it("clears the last attempt's answer when the next one starts", async () => {
    let attempt = 0;
    const { result } = renderHook(() =>
      useGitAction(() => {
        attempt += 1;
        return attempt === 1
          ? Promise.reject(new ApiError(422, "merge conflict", ["src/one.ts"]))
          : Promise.resolve("b");
      }),
    );

    act(() => {
      result.current.submit();
    });
    await waitFor(() => {
      expect(result.current.conflict).not.toBeNull();
    });

    act(() => {
      result.current.submit();
    });
    await waitFor(() => {
      expect(result.current.result).toBe("b");
    });
    expect(result.current.conflict).toBeNull();
  });
});

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
