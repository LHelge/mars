// A force push is destructive and a rejected push is not a failure, so both
// are asserted here: `force` leaves the browser only when it was ticked, and a
// GitHub remote earns the compare link the operator goes to next.

import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ApiError } from "../../services/apiClient";
import { push } from "../../services/git";
import { PushForm } from "./PushForm";

vi.mock("../../services/git", async () => {
  const actual =
    await vi.importActual<typeof import("../../services/git")>(
      "../../services/git",
    );
  return { ...actual, push: vi.fn() };
});

const pushMock = vi.mocked(push);

// Obviously fake fixture values (CLAUDE.md, rule 3).
const PROJECT_ID = "00000000-0000-4000-8000-0000000000b2";
const SESSION_ID = "00000000-0000-4000-8000-0000000000a1";
const REMOTE = "https://github.com/owner/repo.git";

function mount(remoteUrl = REMOTE) {
  const onPushed = vi.fn();
  // The remote-branch hint's `Learn more` is a router link.
  render(
    <MemoryRouter>
      <PushForm
        projectId={PROJECT_ID}
        gitRef={SESSION_ID}
        isSession
        refLabel="Fix the login redirect"
        remoteUrl={remoteUrl}
        compareTarget="main"
        formId="push-test"
        disabled={false}
        onBusy={vi.fn()}
        onPushed={onPushed}
      />
    </MemoryRouter>,
  );
  return onPushed;
}

function submit() {
  fireEvent.click(screen.getByRole("button", { name: "Push" }));
}

beforeEach(() => {
  pushMock.mockReset();
  pushMock.mockResolvedValue({
    remote_branch: `session/${SESSION_ID}`,
    commit: "0123456789abcdef0123456789abcdef01234567",
  });
});

afterEach(cleanup);

describe("PushForm", () => {
  it("defaults the remote branch to session/<id> and omits force", async () => {
    mount();
    submit();

    await waitFor(() => {
      expect(pushMock).toHaveBeenCalledWith(PROJECT_ID, {
        ref: SESSION_ID,
        remote_branch: `session/${SESSION_ID}`,
      });
    });
  });

  it("sends force only once it is ticked", async () => {
    mount();
    fireEvent.click(screen.getByLabelText("Force push"));
    submit();

    await waitFor(() => {
      expect(pushMock).toHaveBeenCalledWith(PROJECT_ID, {
        ref: SESSION_ID,
        remote_branch: `session/${SESSION_ID}`,
        force: true,
      });
    });
    expect(
      screen.getByText(/A force push overwrites whatever is on the remote/),
    ).toBeTruthy();
  });

  it("offers the compare page of a github remote after a push", async () => {
    const onPushed = mount();
    submit();

    const link = await screen.findByRole("link", {
      name: "Open compare on GitHub",
    });
    expect(link.getAttribute("href")).toBe(
      `https://github.com/owner/repo/compare/main...session/${SESSION_ID}?expand=1`,
    );
    expect(screen.getByText(/Pushed session\//)).toBeTruthy();
    expect(onPushed).toHaveBeenCalledTimes(1);
  });

  it("offers no compare link for a remote that is not on github", async () => {
    mount("https://gitlab.com/owner/repo.git");
    submit();

    await screen.findByText(/Pushed session\//);
    expect(
      screen.queryByRole("link", { name: "Open compare on GitHub" }),
    ).toBeNull();
  });

  it("trims the remote branch once, for the request and the enable check", async () => {
    mount();
    fireEvent.change(screen.getByLabelText("Remote branch"), {
      target: { value: "  topic  " },
    });
    submit();

    await waitFor(() => {
      expect(pushMock).toHaveBeenCalledWith(PROJECT_ID, {
        ref: SESSION_ID,
        remote_branch: "topic",
      });
    });
  });

  it("explains a rejected push as the only answer on screen", async () => {
    pushMock.mockRejectedValue(new ApiError(409, "non-fast-forward"));
    mount();
    submit();

    expect(
      await screen.findByText(/Push rejected: upstream has advanced\./),
    ).toBeTruthy();
    // A handled 409 returns, so the server's text does not raise a second
    // alert beside the advice.
    expect(screen.queryByText("non-fast-forward")).toBeNull();
  });

  it("names the branch that was rejected, not the one now typed", async () => {
    pushMock.mockRejectedValue(new ApiError(409, "non-fast-forward"));
    mount();
    fireEvent.change(screen.getByLabelText("Remote branch"), {
      target: { value: "topic" },
    });
    submit();

    await screen.findByText(/merge origin\/topic and retry\./);

    fireEvent.change(screen.getByLabelText("Remote branch"), {
      target: { value: "something-else" },
    });
    expect(screen.getByText(/merge origin\/topic and retry\./)).toBeTruthy();
  });
});
