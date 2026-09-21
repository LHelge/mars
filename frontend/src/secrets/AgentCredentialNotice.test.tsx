// The one sentence the notice exists to say (`SPEC.md`, "Frontend", Agent
// credentials): whose credential won, in the wording of the scope it came
// from, and the warning when there is none.
//
// The scopes are asserted through the rendered component rather than through a
// helper, because the wording *is* the component: "your", "the project's" and
// "the shared" are three different sentences about the same field.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { getAgentCredentials } from "../services/secrets";
import type { AgentCredentialStatus, SecretScope } from "../types";
import { AgentCredentialNotice } from "./AgentCredentialNotice";

vi.mock("../services/secrets", () => ({ getAgentCredentials: vi.fn() }));

// Obviously fake fixture values (CLAUDE.md, rule 3).
const PROJECT_ID = "00000000-0000-4000-8000-0000000000c3";
const SECRET_ID = "00000000-0000-4000-8000-0000000000d4";

const mocked = vi.mocked(getAgentCredentials);

function answer(statuses: AgentCredentialStatus[]) {
  mocked.mockResolvedValue(statuses);
}

function credentialAt(
  scope: SecretScope,
  name = "CLAUDE_CODE_OAUTH_TOKEN",
): AgentCredentialStatus[] {
  return [
    { backend: "claude", credential: { secret_id: SECRET_ID, name, scope } },
  ];
}

function mount(backend = "claude") {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
        <AgentCredentialNotice projectId={PROJECT_ID} backend={backend} />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  mocked.mockReset();
});

afterEach(() => {
  cleanup();
});

describe("AgentCredentialNotice", () => {
  it("names a user-scoped credential as the caller's own", async () => {
    answer(credentialAt("user"));
    mount();

    expect(
      await screen.findByText("Authenticates with your Claude subscription token"),
    ).toBeDefined();
  });

  it("names a project-scoped credential as the project's", async () => {
    answer(credentialAt("project", "ANTHROPIC_API_KEY"));
    mount();

    expect(
      await screen.findByText(
        "Authenticates with the project's Anthropic API key",
      ),
    ).toBeDefined();
  });

  it("names a global credential as the shared one", async () => {
    answer(credentialAt("global"));
    mount();

    expect(
      await screen.findByText(
        "Authenticates with the shared Claude subscription token",
      ),
    ).toBeDefined();
  });

  it("falls back to the stored name for a credential this build has no label for", async () => {
    answer(credentialAt("user", "SOME_FUTURE_TOKEN"));
    mount();

    expect(
      await screen.findByText("Authenticates with your SOME_FUTURE_TOKEN"),
    ).toBeDefined();
  });

  it("warns, with a link to the secrets page, when there is none", async () => {
    answer([{ backend: "claude", credential: null }]);
    mount();

    expect(
      await screen.findByText(/No agent credential: sessions of this profile/),
    ).toBeDefined();
    expect(screen.getByRole("link", { name: "Add one" }).getAttribute("href")).toBe(
      "/secrets",
    );
  });

  it("says nothing about a backend the answer has no entry for", async () => {
    answer([{ backend: "claude", credential: null }]);
    mount("some-future-backend");

    // The answer arrives and still resolves to nothing: not the warning, and
    // not a sentence either (`SPEC.md`, "Frontend": an unknown backend renders
    // nothing).
    await vi.waitFor(() => {
      expect(mocked).toHaveBeenCalled();
    });
    expect(screen.queryByText(/No agent credential/)).toBeNull();
    expect(screen.queryByText(/Authenticates with/)).toBeNull();
  });

  it("says nothing while the answer is on its way, and nothing when it fails", async () => {
    mocked.mockRejectedValue(new Error("nope"));
    mount();

    await vi.waitFor(() => {
      expect(mocked).toHaveBeenCalled();
    });
    expect(screen.queryByText(/No agent credential/)).toBeNull();
    expect(screen.queryByText(/Authenticates with/)).toBeNull();
  });
});
