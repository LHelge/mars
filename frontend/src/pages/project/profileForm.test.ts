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
  idleTimeoutHint,
  maxConcurrentError,
  mergeSecretOptions,
  MIN_IDLE_TIMEOUT_SECS,
  MIN_MAX_CONCURRENT,
  nextFreeName,
  partialMessagesDecided,
  prefillFromTemplate,
  scheduleErrorField,
  scheduleErrors,
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
  schedule_cron: "0 6 * * *",
  schedule_prompt: "Scan the repository for tech debt.",
  last_scheduled_at: "2026-01-02T06:00:00Z",
  next_scheduled_at: "2026-01-03T06:00:00Z",
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

    expect(
      defaultInputForKind("conversational", "img:1").serves_states,
    ).toEqual(["ready"]);
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
      schedule_cron: "0 6 * * *",
      schedule_prompt: "Scan the repository for tech debt.",
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

  it("leaves the scheduler's own timestamps out of the body", () => {
    const input = toProfileInput(STORED);

    expect(input).not.toHaveProperty("last_scheduled_at");
    expect(input).not.toHaveProperty("next_scheduled_at");
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
    expect(
      toFormState({ name: "one", kind: "ephemeral" }).partial_messages,
    ).toBe(false);
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
    expect(partialMessagesDecided({ ...STORED, kind: "conversational" })).toBe(
      false,
    );
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

describe("idleTimeoutHint", () => {
  // `ARCHITECTURE.md`, "Stop semantics": the reaper parks one kind and fails
  // the other, and idle is silence, not the absence of a turn.
  it("parks a conversational session", () => {
    const hint = idleTimeoutHint("conversational");
    expect(hint).toContain("parked");
    expect(hint).not.toContain("stalled");
    expect(hint).toContain("counts as idle");
  });

  it("fails an ephemeral session as stalled", () => {
    const hint = idleTimeoutHint("ephemeral");
    expect(hint).toContain("stalled");
    expect(hint).not.toContain("parked");
    expect(hint).toContain("counts as idle");
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
    schedule_cron: null,
    schedule_prompt: null,
  };

  /** The scheduled template of `SPEC.md`, "Role profile templates". */
  const SCHEDULED: ProfileTemplate = {
    name: "tech-debt-scanner",
    kind: "ephemeral",
    backend: "claude",
    serves_states: ["ready"],
    mcp_tools: [],
    system_prompt: "You are the tech-debt scanner of this project.",
    is_default: false,
    schedule_cron: "0 4 * * *",
    schedule_prompt: "Scan this project for technical debt.",
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

  it("leaves the schedule empty for a template that has none", () => {
    const { form } = prefillFromTemplate(TEMPLATE, {
      defaultImage: "img:1",
      existingNames: [],
      queueStates: ["review"],
    });

    expect(form.schedule_cron).toBe("");
    expect(form.schedule_prompt).toBe("");
    expect(toInput(form).schedule_cron).toBeNull();
    expect(toInput(form).schedule_prompt).toBeNull();
  });

  it("pre-fills the schedule of a scheduled template", () => {
    const { form } = prefillFromTemplate(SCHEDULED, {
      defaultImage: "img:1",
      existingNames: [],
      queueStates: ["ready"],
    });

    expect(form.kind).toBe("ephemeral");
    // The template's schedule arrives switched on.
    expect(form.scheduled).toBe(true);
    expect(form.schedule_cron).toBe("0 4 * * *");
    expect(form.schedule_prompt).toBe("Scan this project for technical debt.");
    // Partial messages follow the kind the template brought, not the
    // conversational default.
    expect(form.partial_messages).toBe(false);
    // Nothing local refuses the pair, and both go on the wire together.
    expect(scheduleErrors(form)).toEqual({ cron: null, prompt: null });
    const input = toInput(form);
    expect(input.schedule_cron).toBe("0 4 * * *");
    expect(input.schedule_prompt).toBe("Scan this project for technical debt.");
    // A schedule is not an auto-launch: the template turns on neither.
    expect(input.auto_launch).toBe(false);
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
    expect(
      toInput(toFormState({ name: "one", max_concurrent: 2 })).max_concurrent,
    ).toBe(2);
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

describe("the schedule pair", () => {
  const ephemeral = (cron: string, prompt: string) =>
    toFormState({
      name: "scanner",
      kind: "ephemeral",
      schedule_cron: cron,
      schedule_prompt: prompt,
    });
  /** The checkbox on, over fields that may still be empty. */
  const switchedOn = (cron: string, prompt: string) => ({
    ...ephemeral("", ""),
    scheduled: true,
    schedule_cron: cron,
    schedule_prompt: prompt,
  });

  it("round-trips a stored schedule through the form, with the checkbox on", () => {
    const state = toFormState(toProfileInput(STORED));

    expect(state.scheduled).toBe(true);
    expect(state.schedule_cron).toBe("0 6 * * *");
    expect(state.schedule_prompt).toBe("Scan the repository for tech debt.");
    expect(toInput(state).schedule_cron).toBe("0 6 * * *");
    expect(toInput(state).schedule_prompt).toBe(
      "Scan the repository for tech debt.",
    );
  });

  it("starts with the checkbox off for a profile with no schedule", () => {
    expect(ephemeral("", "").scheduled).toBe(false);
  });

  it("sends both fields as null while the checkbox is off, whatever the boxes hold", () => {
    const off = { ...ephemeral("0 6 * * *", "Scan."), scheduled: false };
    const input = toInput(off);

    expect(input.schedule_cron).toBeNull();
    expect(input.schedule_prompt).toBeNull();
    // The boxes keep what was typed, so ticking the box again restores it.
    expect(toInput({ ...off, scheduled: true }).schedule_cron).toBe(
      "0 6 * * *",
    );
  });

  it("treats a whitespace-only prompt as no prompt", () => {
    expect(
      toInput(switchedOn("0 6 * * *", "   \n")).schedule_prompt,
    ).toBeNull();
  });

  it("keeps the whitespace inside a prompt that was written", () => {
    expect(
      toInput(switchedOn("0 6 * * *", "Do this.\n\nThen that."))
        .schedule_prompt,
    ).toBe("Do this.\n\nThen that.");
  });

  it("trims the expression, which is never prose", () => {
    expect(toInput(switchedOn("  0 6 * * *  ", "Scan.")).schedule_cron).toBe(
      "0 6 * * *",
    );
  });

  it("clears the schedule for a conversational profile before submit", () => {
    const scheduled = ephemeral("0 6 * * *", "Scan.");
    const input = toInput({ ...scheduled, kind: "conversational" });

    expect(input.schedule_cron).toBeNull();
    expect(input.schedule_prompt).toBeNull();
    // The intent stays in the form, so switching back restores it.
    expect(toInput(scheduled).schedule_cron).toBe("0 6 * * *");
  });

  it("requires both fields once the checkbox is on", () => {
    expect(scheduleErrors(switchedOn("", ""))).toEqual({
      cron: "Required.",
      prompt: "Required.",
    });
    expect(scheduleErrors(switchedOn("0 6 * * *", "")).prompt).not.toBeNull();
    expect(scheduleErrors(switchedOn("0 6 * * *", "")).cron).toBeNull();
    expect(scheduleErrors(switchedOn("", "Scan.")).cron).not.toBeNull();
    expect(scheduleErrors(switchedOn("", "Scan.")).prompt).toBeNull();
    expect(scheduleErrors(switchedOn("0 6 * * *", "Scan."))).toEqual({
      cron: null,
      prompt: null,
    });
  });

  it("refuses nothing while the checkbox is off", () => {
    // Half-filled boxes under an unticked checkbox are not sent, so they are
    // not a mistake either.
    expect(
      scheduleErrors({ ...ephemeral("0 6 * * *", ""), scheduled: false }),
    ).toEqual({
      cron: null,
      prompt: null,
    });
  });

  it("judges no expression itself", () => {
    // Nonsense that only the server can refuse still passes the local check:
    // the bundle carries no cron parser (ADR 0043).
    expect(scheduleErrors(switchedOn("@daily", "Scan.")).cron).toBeNull();
    expect(
      scheduleErrors(switchedOn("not cron at all", "Scan.")).cron,
    ).toBeNull();
  });
});

describe("scheduleErrorField", () => {
  // The six refusals of `SPEC.md`, "Agent profiles" → "Scheduled profiles",
  // word for word, so a reworded one fails here rather than silently landing
  // in the form's own alert.
  it("routes every documented refusal to its own field", () => {
    expect(
      scheduleErrorField(
        "schedule_cron must be a 5-field cron expression (minute hour day-of-month month day-of-week) evaluated in UTC, with no seconds field, no year field and no @-form",
      ),
    ).toBe("cron");
    expect(
      scheduleErrorField(
        "schedule_cron is not a valid cron expression: unexpected token",
      ),
    ).toBe("cron");
    expect(
      scheduleErrorField("schedule_cron requires an ephemeral profile"),
    ).toBe("cron");
    expect(
      scheduleErrorField(
        "schedule_cron requires this backend's agent credential at global or project scope",
      ),
    ).toBe("cron");
    expect(
      scheduleErrorField(
        "schedule_prompt is required when schedule_cron is set",
      ),
    ).toBe("prompt");
    expect(scheduleErrorField("schedule_prompt requires schedule_cron")).toBe(
      "prompt",
    );
    expect(
      scheduleErrorField("schedule prompt must be at most 65536 bytes"),
    ).toBe("prompt");
  });

  it("leaves every other refusal to the form", () => {
    expect(
      scheduleErrorField("auto_launch requires an ephemeral profile"),
    ).toBeNull();
    expect(scheduleErrorField("max_concurrent must be at least 1")).toBeNull();
    expect(scheduleErrorField("name is already taken")).toBeNull();
  });
});
