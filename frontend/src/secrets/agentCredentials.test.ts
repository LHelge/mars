import { describe, expect, it } from "vitest";
import {
  AGENT_CREDENTIALS,
  ALL_AGENT_CREDENTIALS,
  credentialScope,
  labelForCredential,
} from "./agentCredentials";

// Obviously fake (`CLAUDE.md`, rule 3).
const PROJECT_ID = "00000000-0000-0000-0000-0000000000a1";

describe("the credential table", () => {
  it("names the two Claude credentials, the subscription token first", () => {
    expect(AGENT_CREDENTIALS.claude.map((entry) => entry.name)).toEqual([
      "CLAUDE_CODE_OAUTH_TOKEN",
      "ANTHROPIC_API_KEY",
    ]);
  });

  it("labels each of them, and hints where each comes from", () => {
    const [token, key] = AGENT_CREDENTIALS.claude;
    expect(token?.label).toBe("Claude subscription token");
    expect(token?.hint).toContain("claude setup-token");
    expect(token?.hint).toContain("Pro or Max");
    expect(key?.label).toBe("Anthropic API key");
    expect(key?.hint).toContain("Anthropic Console");
    expect(key?.hint).toContain("billed per use");
  });

  it("flattens every backend's credentials into one list", () => {
    expect(ALL_AGENT_CREDENTIALS).toEqual(AGENT_CREDENTIALS.claude);
  });
});

describe("labelForCredential", () => {
  it("answers the label of a known name", () => {
    expect(labelForCredential("CLAUDE_CODE_OAUTH_TOKEN")).toBe(
      "Claude subscription token",
    );
    expect(labelForCredential("ANTHROPIC_API_KEY")).toBe("Anthropic API key");
  });

  it("answers the name itself for anything else", () => {
    // A credential the server knows and this build does not is still shown.
    expect(labelForCredential("SOME_FUTURE_TOKEN")).toBe("SOME_FUTURE_TOKEN");
  });
});

describe("credentialScope", () => {
  it("maps `Me` onto the caller's user scope, with no id", () => {
    expect(credentialScope("me", null)).toEqual({ scope: "user" });
    // A selected project is irrelevant while `Me` is chosen.
    expect(credentialScope("me", PROJECT_ID)).toEqual({ scope: "user" });
  });

  it("maps `Everyone` onto the global scope", () => {
    expect(credentialScope("everyone", null)).toEqual({ scope: "global" });
  });

  it("maps `A project` onto that project's scope", () => {
    expect(credentialScope("project", PROJECT_ID)).toEqual({
      scope: "project",
      scope_id: PROJECT_ID,
    });
  });

  it("has no scope for `A project` until one is chosen", () => {
    expect(credentialScope("project", null)).toBeNull();
    expect(credentialScope("project", "")).toBeNull();
  });
});
