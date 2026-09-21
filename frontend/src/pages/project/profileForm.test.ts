import { describe, expect, it } from "vitest";
import type {
  AgentCredential,
  Profile,
  ProfileTemplate,
  SecretMeta,
} from "../../types";
import {
  DEFAULT_IDLE_TIMEOUT_SECS,
  defaultInputForKind,
  idleTimeoutError,
  maxConcurrentError,
  mergeSecretOptions,
  MIN_IDLE_TIMEOUT_SECS,
  MIN_MAX_CONCURRENT,
  nextFreeName,
  partialMessagesDecided,
  prefillFromTemplate,
  toFormState,
  toggleMember,
  toInput,
  toProfileInput,
  unattendedCredential,
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
  auto_launch: true,
  max_concurrent: 3,
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
      auto_launch: true,
      max_concurrent: 3,
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

describe("partialMessagesDecided", () => {
  it("has decided nothing for a new profile", () => {
    expect(partialMessagesDecided(null)).toBe(false);
  });

  it("leaves a stored answer that differs from its kind's default alone", () => {
    // The editor's `partial_messages` follows the kind only until somebody has
    // decided: an ephemeral profile streaming partial messages, or this
    // conversational one deliberately not, has. Switching kind and back must
    // not quietly put the default answer back.
    expect(partialMessagesDecided({ ...STORED, kind: "ephemeral" })).toBe(true);
    expect(
      partialMessagesDecided({
        ...STORED,
        kind: "conversational",
        partial_messages: false,
      }),
    ).toBe(true);
  });

  it("lets the kind keep deciding when the stored value is its default", () => {
    expect(
      partialMessagesDecided({
        ...STORED,
        kind: "ephemeral",
        partial_messages: false,
      }),
    ).toBe(false);
    expect(
      partialMessagesDecided({ ...STORED, kind: "conversational" }),
    ).toBe(false);
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
  });

  it("accepts every timeout the API does, so a stored one can be saved back", () => {
    // `SPEC.md`, "Agent profiles": `idle_timeout_secs` at least 1. A profile
    // created over the API or by an agent with a short timeout is editable
    // without having to be given a longer one first.
    expect(MIN_IDLE_TIMEOUT_SECS).toBe(1);
    expect(idleTimeoutError("30")).toBeNull();
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

describe("nextFreeName", () => {
  it("keeps the template's own name when nothing has taken it", () => {
    expect(nextFreeName("reviewer", ["planner", "implementer"])).toBe(
      "reviewer",
    );
  });

  it("suffixes the first free number when the name is taken", () => {
    expect(nextFreeName("reviewer", ["reviewer"])).toBe("reviewer-2");
    expect(nextFreeName("reviewer", ["reviewer", "reviewer-2"])).toBe(
      "reviewer-3",
    );
  });

  it("skips over a gap rather than reusing a taken suffix", () => {
    expect(nextFreeName("reviewer", ["reviewer", "reviewer-3"])).toBe(
      "reviewer-2",
    );
  });
});

describe("prefillFromTemplate", () => {
  const TEMPLATE: ProfileTemplate = {
    name: "reviewer",
    kind: "conversational",
    backend: "claude",
    serves_states: ["review"],
    mcp_tools: ["list_session_branches"],
    system_prompt: "You are a reviewer of this project.",
    is_default: true,
  };

  it("fills name, states, tools and prompt and leaves the rest at the defaults", () => {
    const { form, droppedStates } = prefillFromTemplate(TEMPLATE, {
      defaultImage: "example.invalid/mars/session:1",
      existingNames: [],
      queueStates: ["backlog", "ready", "review", "merge"],
    });

    expect(form.name).toBe("reviewer");
    expect(form.serves_states).toEqual(["review"]);
    expect(form.mcp_tools).toEqual(["list_session_branches"]);
    expect(form.system_prompt).toBe("You are a reviewer of this project.");
    expect(droppedStates).toEqual([]);

    // Everything the template does not carry is a new profile's default.
    expect(form.kind).toBe("conversational");
    expect(form.image).toBe("example.invalid/mars/session:1");
    expect(form.model).toBe("");
    expect(form.runtime).toBe("");
    expect(form.secrets).toEqual([]);
    expect(form.partial_messages).toBe(true);
    expect(form.idle_timeout_secs).toBe(String(DEFAULT_IDLE_TIMEOUT_SECS));
  });

  it("suffixes the name when the project already has that role", () => {
    const { form } = prefillFromTemplate(TEMPLATE, {
      defaultImage: "img:1",
      existingNames: ["planner", "reviewer"],
      queueStates: ["review"],
    });

    expect(form.name).toBe("reviewer-2");
  });

  it("drops a served state this project has no queue state for", () => {
    const { form, droppedStates } = prefillFromTemplate(
      { ...TEMPLATE, serves_states: ["review", "triage"] },
      {
        defaultImage: "img:1",
        existingNames: [],
        queueStates: ["ready", "review"],
      },
    );

    expect(form.serves_states).toEqual(["review"]);
    expect(droppedStates).toEqual(["triage"]);
  });

  it("keeps the template's states while the project's own are unknown", () => {
    const { form, droppedStates } = prefillFromTemplate(TEMPLATE, {
      defaultImage: "img:1",
      existingNames: [],
      queueStates: null,
    });

    expect(form.serves_states).toEqual(["review"]);
    expect(droppedStates).toEqual([]);
  });

  it("never carries the template's is_default into the form", () => {
    const { form } = prefillFromTemplate(TEMPLATE, {
      defaultImage: "img:1",
      existingNames: [],
      queueStates: ["review"],
    });

    expect(form).not.toHaveProperty("is_default");
  });
});

describe("auto-launch and the concurrency cap", () => {
  it("sends both on every save, so a PUT cannot reset them", () => {
    const input = toInput(
      toFormState({
        name: "one",
        kind: "ephemeral",
        auto_launch: true,
        max_concurrent: 4,
      }),
    );

    expect(input.auto_launch).toBe(true);
    expect(input.max_concurrent).toBe(4);
  });

  it("defaults to off and one when the input leaves them out", () => {
    const state = toFormState({ name: "one", kind: "ephemeral" });

    expect(state.auto_launch).toBe(false);
    expect(state.max_concurrent).toBe(String(MIN_MAX_CONCURRENT));
  });

  it("clears auto-launch for a conversational profile before submit", () => {
    const ephemeral = toFormState({
      name: "one",
      kind: "ephemeral",
      auto_launch: true,
    });

    expect(toInput({ ...ephemeral, kind: "conversational" }).auto_launch).toBe(
      false,
    );
    // The intent stays in the form, so switching back restores it.
    expect(toInput(ephemeral).auto_launch).toBe(true);
  });

  it("still sends the cap for a conversational profile", () => {
    expect(toInput(toFormState({ name: "one", max_concurrent: 2 })).max_concurrent).toBe(2);
  });

  it("refuses a cap below one, a blank one and a fraction", () => {
    expect(maxConcurrentError("0")).toBe("At least 1.");
    expect(maxConcurrentError("")).toBe("A whole number.");
    expect(maxConcurrentError("1.5")).toBe("A whole number.");
    expect(maxConcurrentError("1")).toBeNull();
    expect(maxConcurrentError("12")).toBeNull();
  });
});

describe("unattendedCredential", () => {
  const at = (scope: AgentCredential["scope"]): AgentCredential => ({
    secret_id: "33333333-3333-4333-8333-333333333333",
    name: "CLAUDE_CODE_OAUTH_TOKEN",
    scope,
  });

  it("resolves for a credential that needs no user", () => {
    expect(unattendedCredential(at("global"))).toBe("resolves");
    expect(unattendedCredential(at("project"))).toBe("resolves");
  });

  it("does not resolve for one that belongs to a user", () => {
    expect(unattendedCredential(at("user"))).toBe("user_only");
  });

  it("separates no credential from no answer yet", () => {
    expect(unattendedCredential(null)).toBe("missing");
    expect(unattendedCredential(undefined)).toBe("unknown");
  });
});
