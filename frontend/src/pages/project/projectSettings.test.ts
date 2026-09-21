import { describe, expect, it } from "vitest";
import type { Project } from "../../types";
import {
  maxAttemptsError,
  sessionCapError,
  settingsBlocked,
  toProjectUpdate,
  toSettingsState,
} from "./projectSettings";

const PROJECT: Project = {
  id: "11111111-1111-4111-8111-111111111111",
  name: "mars",
  remote_url: "git@example.invalid:acme/mars.git",
  default_branch: "main",
  status: "ready",
  status_message: null,
  last_fetched_at: "2026-01-01T00:00:00Z",
  max_attempts: 3,
  max_concurrent_sessions: 4,
  automation_paused: true,
  created_at: "2026-01-01T00:00:00Z",
  has_credential: true,
};

describe("toSettingsState", () => {
  it("shows the stored cap and the pause as they are", () => {
    const state = toSettingsState(PROJECT);

    expect(state.max_concurrent_sessions).toBe("4");
    expect(state.automation_paused).toBe(true);
  });

  it("shows no cap as an empty field", () => {
    expect(
      toSettingsState({ ...PROJECT, max_concurrent_sessions: null })
        .max_concurrent_sessions,
    ).toBe("");
  });
});

describe("sessionCapError", () => {
  it("accepts an empty field, which is no cap", () => {
    expect(sessionCapError("")).toBeNull();
    expect(sessionCapError("   ")).toBeNull();
  });

  it("refuses a cap below one and one that is not whole", () => {
    expect(sessionCapError("0")).toBe("At least 1, or empty for no cap.");
    expect(sessionCapError("2.5")).toBe("A whole number, or empty for no cap.");
    expect(sessionCapError("1")).toBeNull();
  });
});

describe("maxAttemptsError", () => {
  it("holds the endpoint's 1–20", () => {
    expect(maxAttemptsError("0")).not.toBeNull();
    expect(maxAttemptsError("21")).not.toBeNull();
    expect(maxAttemptsError("")).not.toBeNull();
    expect(maxAttemptsError("20")).toBeNull();
  });
});

describe("toProjectUpdate", () => {
  it("sends an explicit null to remove the cap", () => {
    const body = toProjectUpdate({
      ...toSettingsState(PROJECT),
      max_concurrent_sessions: "",
    });

    // The key is present and null: omitting it would leave the stored cap
    // alone (`SPEC.md`, "Projects").
    expect("max_concurrent_sessions" in body).toBe(true);
    expect(body.max_concurrent_sessions).toBeNull();
  });

  it("sends the cap and the pause as numbers and booleans", () => {
    const body = toProjectUpdate(toSettingsState(PROJECT));

    expect(body.max_concurrent_sessions).toBe(4);
    expect(body.automation_paused).toBe(true);
    expect(body.max_attempts).toBe(3);
    expect(body.default_branch).toBe("main");
  });

  it("leaves the branch out while it is still being discovered", () => {
    const body = toProjectUpdate(
      toSettingsState({ ...PROJECT, default_branch: null }),
    );

    expect("default_branch" in body).toBe(false);
  });
});

describe("settingsBlocked", () => {
  it("blocks on a name, an attempt limit or a cap the endpoint would refuse", () => {
    const ok = toSettingsState(PROJECT);

    expect(settingsBlocked(ok)).toBe(false);
    expect(settingsBlocked({ ...ok, name: "  " })).toBe(true);
    expect(settingsBlocked({ ...ok, max_attempts: "0" })).toBe(true);
    expect(settingsBlocked({ ...ok, max_concurrent_sessions: "0" })).toBe(true);
    expect(settingsBlocked({ ...ok, max_concurrent_sessions: "" })).toBe(false);
  });
});
