import { describe, expect, it } from "vitest";
import type { Profile, SecretMeta } from "../../types";
import {
  DEFAULT_IDLE_TIMEOUT_SECS,
  defaultInputForKind,
  idleTimeoutError,
  mergeSecretOptions,
  MIN_IDLE_TIMEOUT_SECS,
  toFormState,
  toggleMember,
  toInput,
  toProfileInput,
} from "./profileForm";

const STORED: Profile = {
  id: "11111111-1111-4111-8111-111111111111",
  project_id: "22222222-2222-4222-8222-222222222222",
  name: "reviewer",
  kind: "ephemeral",
  backend: "claude",
  model: "claude-sonnet-4",
  system_prompt: "Review the hand-off.",
  permission_mode: "bypass",
  image: "example.invalid/mars/session:1",
  runtime: "runsc",
  mcp_tools: ["merge", "push"],
  secrets: ["EXAMPLE_TOKEN"],
  serves_states: ["review"],
  partial_messages: true,
  idle_timeout_secs: 600,
  is_default: true,
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-02T00:00:00Z",
};

describe("defaultInputForKind", () => {
  it("streams partial messages for a conversational profile", () => {
    const input = defaultInputForKind("conversational", "img:1");

    expect(input.partial_messages).toBe(true);
    expect(input.kind).toBe("conversational");
  });

  it("does not stream partial messages for an ephemeral profile", () => {
    expect(defaultInputForKind("ephemeral", "img:1").partial_messages).toBe(
      false,
    );
  });

  it("starts on the image it was given and the documented defaults", () => {
    const input = defaultInputForKind("conversational", "img:1");

    expect(input.image).toBe("img:1");
    expect(input.serves_states).toEqual(["ready"]);
    expect(input.idle_timeout_secs).toBe(DEFAULT_IDLE_TIMEOUT_SECS);
    expect(input.permission_mode).toBe("bypass");
    expect(input.backend).toBe("claude");
    expect(input.mcp_tools).toEqual([]);
    expect(input.secrets).toEqual([]);
    expect(input.model).toBeNull();
    expect(input.runtime).toBeNull();
  });

  it("hands out a fresh state array each time", () => {
    const first = defaultInputForKind("conversational", "img:1");
    first.serves_states?.push("backlog");

    expect(defaultInputForKind("conversational", "img:1").serves_states).toEqual(
      ["ready"],
    );
  });
});

describe("toProfileInput", () => {
  it("keeps every settable field", () => {
    expect(toProfileInput(STORED)).toEqual({
      name: "reviewer",
      kind: "ephemeral",
      backend: "claude",
      model: "claude-sonnet-4",
      system_prompt: "Review the hand-off.",
      permission_mode: "bypass",
      image: "example.invalid/mars/session:1",
      runtime: "runsc",
      mcp_tools: ["merge", "push"],
      secrets: ["EXAMPLE_TOKEN"],
      serves_states: ["review"],
      partial_messages: true,
      idle_timeout_secs: 600,
    });
  });

  it("strips the ids, the timestamps and is_default", () => {
    const input = toProfileInput(STORED);

    expect(input).not.toHaveProperty("id");
    expect(input).not.toHaveProperty("project_id");
    expect(input).not.toHaveProperty("is_default");
    expect(input).not.toHaveProperty("created_at");
    expect(input).not.toHaveProperty("updated_at");
  });

  it("copies the arrays rather than aliasing the profile's", () => {
    const input = toProfileInput(STORED);
    input.mcp_tools?.push("rebase");

    expect(STORED.mcp_tools).toEqual(["merge", "push"]);
  });
});

describe("toFormState and toInput", () => {
  it("round-trips a stored profile unchanged", () => {
    expect(toInput(toFormState(toProfileInput(STORED)))).toEqual(
      toProfileInput(STORED),
    );
  });

  it("shows an unset optional as an empty field and sends it back as null", () => {
    const state = toFormState({ name: "planner" });

    expect(state.model).toBe("");
    expect(state.runtime).toBe("");
    expect(state.system_prompt).toBe("");
    expect(state.serves_states).toEqual(["ready"]);
    expect(state.idle_timeout_secs).toBe(String(DEFAULT_IDLE_TIMEOUT_SECS));

    const input = toInput(state);
    expect(input.model).toBeNull();
    expect(input.runtime).toBeNull();
    expect(input.system_prompt).toBeNull();
  });

  it("defaults partial messages to the kind when the input leaves it out", () => {
    expect(toFormState({ name: "one", kind: "ephemeral" }).partial_messages).toBe(
      false,
    );
    expect(toFormState({ name: "two" }).partial_messages).toBe(true);
  });

  it("always sends partial_messages from the checkbox, not the kind", () => {
    const state = toFormState({ name: "one", kind: "ephemeral" });

    expect(toInput({ ...state, partial_messages: true })).toMatchObject({
      kind: "ephemeral",
      partial_messages: true,
    });
  });

  it("trims the name, the image, the model and the runtime", () => {
    const input = toInput({
      ...toFormState({ name: "  planner  " }),
      image: "  img:1  ",
      model: "  opus  ",
      runtime: "  runsc  ",
    });

    expect(input.name).toBe("planner");
    expect(input.image).toBe("img:1");
    expect(input.model).toBe("opus");
    expect(input.runtime).toBe("runsc");
  });
});

describe("idleTimeoutError", () => {
  it("accepts the default", () => {
    expect(idleTimeoutError(String(DEFAULT_IDLE_TIMEOUT_SECS))).toBeNull();
  });

  it("accepts the floor", () => {
    expect(idleTimeoutError(String(MIN_IDLE_TIMEOUT_SECS))).toBeNull();
  });

  it("refuses an empty, fractional or non-numeric value", () => {
    expect(idleTimeoutError("")).not.toBeNull();
    expect(idleTimeoutError("90.5")).not.toBeNull();
    expect(idleTimeoutError("soon")).not.toBeNull();
  });

  it("refuses anything below the floor, including zero and negatives", () => {
    expect(idleTimeoutError("0")).not.toBeNull();
    expect(idleTimeoutError("-60")).not.toBeNull();
    expect(idleTimeoutError("59")).not.toBeNull();
  });
});

describe("toggleMember", () => {
  it("adds a missing entry at the end", () => {
    expect(toggleMember(["ready"], "review")).toEqual(["ready", "review"]);
  });

  it("removes a present entry and keeps the order of the rest", () => {
    expect(toggleMember(["ready", "review", "merge"], "review")).toEqual([
      "ready",
      "merge",
    ]);
  });

  it("does not mutate its argument", () => {
    const list = ["ready"];
    toggleMember(list, "review");

    expect(list).toEqual(["ready"]);
  });
});

describe("mergeSecretOptions", () => {
  /** An ordinary secret; only the fields the picker reads carry meaning. */
  function secret(name: string, extra: Partial<SecretMeta> = {}): SecretMeta {
    return {
      id: `id-${name}`,
      scope: "global",
      scope_id: null,
      name,
      orchestrator_only: false,
      key_version: 1,
      created_by: null,
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-01T00:00:00Z",
      last_used_at: null,
      credential_for: null,
      ...extra,
    };
  }

  it("sorts the names of every scope into one list", () => {
    const options = mergeSecretOptions([
      [secret("ZULU")],
      [secret("ALPHA")],
      undefined,
    ]);

    expect(options.map((option) => option.name)).toEqual(["ALPHA", "ZULU"]);
  });

  it("keeps the later scope's flag when a name is defined twice", () => {
    const options = mergeSecretOptions([
      [secret("SHARED", { orchestrator_only: true })],
      [secret("SHARED")],
    ]);

    expect(options).toEqual([{ name: "SHARED", orchestrator_only: false }]);
  });

  it("leaves out every name the server marked as an agent credential", () => {
    const options = mergeSecretOptions([
      [
        secret("CLAUDE_CODE_OAUTH_TOKEN", { credential_for: "claude" }),
        secret("ANTHROPIC_API_KEY", { credential_for: "claude" }),
        secret("EXAMPLE_TOKEN"),
      ],
    ]);

    expect(options.map((option) => option.name)).toEqual(["EXAMPLE_TOKEN"]);
  });
});
