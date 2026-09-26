import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { INVITE_DELIVERY } from "../components/admin/inviteDelivery";
import { ApiError } from "../services/apiClient";
import { clearAuth, installSession } from "../services/auth";
import {
  createInvite,
  deleteUser,
  listInvites,
  listUsers,
  resendInvite,
  revokeInvite,
  updateUser,
} from "../services/users";
import type { Invite, User } from "../types";
import { AdminPage } from "./AdminPage";

vi.mock("../services/users", () => ({
  createInvite: vi.fn(),
  deleteUser: vi.fn(),
  listInvites: vi.fn(),
  listUsers: vi.fn(),
  resendInvite: vi.fn(),
  revokeInvite: vi.fn(),
  updateUser: vi.fn(),
}));

// Obviously fake fixture values (CLAUDE.md, rule 3).
const ME_ID = "00000000-0000-0000-0000-0000000000a1";
const OTHER_ID = "00000000-0000-0000-0000-0000000000b2";
const THIRD_ID = "00000000-0000-0000-0000-0000000000e5";
const INVITE_ID = "00000000-0000-0000-0000-0000000000c3";

function me(overrides: Partial<User> = {}): User {
  return {
    id: ME_ID,
    username: "admin",
    email: "admin@example.invalid",
    admin: true,
    must_change_password: false,
    notify_email: true,
    created_at: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

function other(overrides: Partial<User> = {}): User {
  return {
    id: OTHER_ID,
    username: "operator",
    email: "operator@example.invalid",
    admin: true,
    must_change_password: true,
    notify_email: false,
    created_at: "2026-02-01T00:00:00Z",
    ...overrides,
  };
}

function third(overrides: Partial<User> = {}): User {
  return {
    id: THIRD_ID,
    username: "zoe",
    email: "zoe@example.invalid",
    admin: false,
    must_change_password: false,
    notify_email: true,
    created_at: "2026-02-15T00:00:00Z",
    ...overrides,
  };
}

function invite(overrides: Partial<Invite> = {}): Invite {
  return {
    id: INVITE_ID,
    email: "newcomer@example.invalid",
    admin: false,
    invited_by: ME_ID,
    expires_at: "2099-01-01T00:00:00Z",
    created_at: "2026-03-01T00:00:00Z",
    ...overrides,
  };
}

function renderAdmin() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={["/admin"]}>
        <AdminPage />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/** The table under a section heading, once it has rendered. */
async function sectionTable(title: string): Promise<HTMLElement> {
  const heading = await screen.findByRole("heading", { name: title });
  const section = heading.closest("section");
  if (section === null) {
    throw new Error(`no section for ${title}`);
  }
  return within(section).findByRole("table");
}

/** The table row a cell's text identifies. */
function rowFor(table: HTMLElement, text: string): HTMLElement {
  const row = within(table)
    .getAllByRole("row")
    .find((candidate) => within(candidate).queryByText(text) !== null);
  if (row === undefined) {
    throw new Error(`no row for ${text}`);
  }
  return row;
}

/**
 * Press a row's action and then the `ConfirmPanel` it opens, by the confirming
 * button's own name — which names its target, so a table of rows offering the
 * same verb still has one button per row.
 */
function confirm(name: string): void {
  fireEvent.click(screen.getByRole("button", { name }));
}

beforeEach(() => {
  vi.mocked(listUsers).mockResolvedValue([me(), other()]);
  vi.mocked(listInvites).mockResolvedValue([invite()]);
  installSession({ user: me(), access_token: "test-access-token" });
});

afterEach(() => {
  cleanup();
  clearAuth();
  vi.restoreAllMocks();
  vi.clearAllMocks();
});

describe("AdminPage", () => {
  it("renders the users and the open invitations", async () => {
    renderAdmin();

    const users = await sectionTable("Users");
    expect(within(users).getByText("operator")).toBeDefined();
    expect(within(users).getByText("admin@example.invalid")).toBeDefined();
    // The signed-in administrator's own row is marked.
    expect(within(users).getByText("(you)")).toBeDefined();
    expect(within(users).getByText("required")).toBeDefined();

    const invites = await sectionTable("Invitations");
    expect(within(invites).getByText("newcomer@example.invalid")).toBeDefined();
    // `invited_by` resolves through the users list.
    expect(within(invites).getByText("admin")).toBeDefined();
  });

  it("disables Delete on the signed-in administrator's own row", async () => {
    renderAdmin();

    const users = await sectionTable("Users");
    const mine = rowFor(users, "(you)");
    const deleteButton: HTMLButtonElement = within(mine).getByRole("button", {
      name: "Delete",
    });

    expect(deleteButton.disabled).toBe(true);
    // The reason is on the button itself, not on a wrapper the keyboard
    // cannot reach (`CLAUDE.md`, "Frontend conventions": shared UI).
    expect(deleteButton.getAttribute("title")).toBe(
      "You cannot delete your own account",
    );
  });

  it("sends both fields when the admin flag is toggled", async () => {
    vi.mocked(updateUser).mockResolvedValue(other({ admin: false }));
    renderAdmin();

    await sectionTable("Users");
    fireEvent.click(
      screen.getByRole("checkbox", { name: "Administrator: operator" }),
    );

    await waitFor(() => {
      expect(vi.mocked(updateUser)).toHaveBeenCalledWith(OTHER_ID, {
        username: "operator",
        admin: false,
      });
    });
  });

  it("confirms before the administrator demotes themselves", async () => {
    renderAdmin();

    await sectionTable("Users");
    fireEvent.click(
      screen.getByRole("checkbox", { name: "Administrator: admin" }),
    );

    // Nothing is sent until the panel's own button is pressed.
    expect(vi.mocked(updateUser)).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(
      screen.queryByRole("button", { name: "Step down as administrator" }),
    ).toBeNull();
    expect(vi.mocked(updateUser)).not.toHaveBeenCalled();

    fireEvent.click(
      screen.getByRole("checkbox", { name: "Administrator: admin" }),
    );
    confirm("Step down as administrator");
    await waitFor(() => {
      expect(vi.mocked(updateUser)).toHaveBeenCalledWith(ME_ID, {
        username: "admin",
        admin: false,
      });
    });
  });

  it("shows the refusal and leaves the checkbox alone when a toggle is rejected", async () => {
    vi.mocked(updateUser).mockRejectedValue(
      new ApiError(409, "cannot demote the last administrator"),
    );
    renderAdmin();

    await sectionTable("Users");
    const checkbox: HTMLInputElement = screen.getByRole("checkbox", {
      name: "Administrator: operator",
    });
    fireEvent.click(checkbox);

    expect(
      await screen.findByText("cannot demote the last administrator"),
    ).toBeDefined();
    // Controlled by the query data, so the rejected change never landed.
    expect(checkbox.checked).toBe(true);
  });

  it("confirms a deletion and shows the server's refusal", async () => {
    vi.mocked(deleteUser).mockRejectedValue(
      new ApiError(409, "cannot delete the last administrator"),
    );
    renderAdmin();

    const users = await sectionTable("Users");
    fireEvent.click(
      within(rowFor(users, "operator")).getByRole("button", { name: "Delete" }),
    );
    confirm("Delete user operator");

    await waitFor(() => {
      expect(vi.mocked(deleteUser)).toHaveBeenCalledWith(OTHER_ID);
    });
    expect(
      await screen.findByText("cannot delete the last administrator"),
    ).toBeDefined();
  });

  it("keeps both rows busy while two deletions overlap", async () => {
    vi.mocked(listUsers).mockResolvedValue([me(), other(), third()]);
    // One resolver per user, so each request can be answered on its own.
    const finish = new Map<string, () => void>();
    vi.mocked(deleteUser).mockImplementation(
      (id: string) =>
        new Promise<void>((resolve) => {
          finish.set(id, () => {
            resolve();
          });
        }),
    );
    renderAdmin();

    const users = await sectionTable("Users");
    const deleteIn = (username: string): HTMLButtonElement =>
      within(rowFor(users, username)).getByRole("button", { name: "Delete" });

    fireEvent.click(deleteIn("operator"));
    confirm("Delete user operator");
    await waitFor(() => {
      expect(finish.has(OTHER_ID)).toBe(true);
    });
    fireEvent.click(deleteIn("zoe"));
    confirm("Delete user zoe");
    await waitFor(() => {
      expect(finish.has(THIRD_ID)).toBe(true);
    });

    // A shared observer would have followed the second press and re-enabled
    // the first row while its DELETE was still in flight.
    expect(deleteIn("operator").disabled).toBe(true);
    expect(deleteIn("zoe").disabled).toBe(true);

    // The row whose request answered stops waiting; the other keeps waiting.
    vi.mocked(listUsers).mockResolvedValue([me(), third()]);
    finish.get(OTHER_ID)?.();
    await waitFor(() => {
      expect(within(users).queryByText("operator")).toBeNull();
    });
    expect(deleteIn("zoe").disabled).toBe(true);
  });

  it("normalises the invited address and says where the link is", async () => {
    const created = invite({
      id: "00000000-0000-0000-0000-0000000000d4",
      email: "newbie@example.invalid",
      admin: true,
    });
    vi.mocked(createInvite).mockResolvedValue(created);
    // The refetch the write triggers lists it too.
    vi.mocked(listInvites)
      .mockResolvedValueOnce([invite()])
      .mockResolvedValue([created, invite()]);
    renderAdmin();

    await sectionTable("Invitations");
    fireEvent.change(screen.getByLabelText(/Email/), {
      target: { value: "  Newbie@Example.Invalid  " },
    });
    fireEvent.click(screen.getByRole("checkbox", { name: /^Administrator$/ }));
    fireEvent.click(screen.getByRole("button", { name: "Send invitation" }));

    await waitFor(() => {
      expect(vi.mocked(createInvite)).toHaveBeenCalledWith({
        email: "newbie@example.invalid",
        admin: true,
      });
    });

    expect(
      await screen.findByText(
        `Invitation created for newbie@example.invalid. ${INVITE_DELIVERY}`,
      ),
    ).toBeDefined();

    // The form is cleared and the new invitation is on the table.
    const field: HTMLInputElement = screen.getByLabelText(/Email/);
    expect(field.value).toBe("");
    const invites = await sectionTable("Invitations");
    expect(within(invites).getByText("newbie@example.invalid")).toBeDefined();
  });

  it("rewords the duplicate-invitation conflict", async () => {
    vi.mocked(createInvite).mockRejectedValue(
      new ApiError(409, "email already invited"),
    );
    renderAdmin();

    await sectionTable("Invitations");
    fireEvent.change(screen.getByLabelText(/Email/), {
      target: { value: "taken@example.invalid" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Send invitation" }));

    expect(
      await screen.findByText(
        "That email already has an account or an open invitation.",
      ),
    ).toBeDefined();
  });

  it("removes the row a revoke succeeded on", async () => {
    vi.mocked(revokeInvite).mockResolvedValue(undefined);
    // The refetch after the write no longer lists it.
    vi.mocked(listInvites)
      .mockResolvedValueOnce([invite()])
      .mockResolvedValue([]);
    renderAdmin();

    const invites = await sectionTable("Invitations");
    fireEvent.click(within(invites).getByRole("button", { name: "Revoke" }));
    confirm("Revoke the invitation for newcomer@example.invalid");

    await waitFor(() => {
      expect(vi.mocked(revokeInvite)).toHaveBeenCalledWith(INVITE_ID);
    });
    expect(await screen.findByText("No open invitations")).toBeDefined();
  });

  it("replaces the row a resend answered with, and says so", async () => {
    vi.mocked(resendInvite).mockResolvedValue(
      invite({ expires_at: "2099-06-01T00:00:00Z" }),
    );
    renderAdmin();

    const invites = await sectionTable("Invitations");
    fireEvent.click(within(invites).getByRole("button", { name: "Resend" }));

    await waitFor(() => {
      expect(vi.mocked(resendInvite)).toHaveBeenCalledWith(INVITE_ID);
    });
    expect(
      await screen.findByText(/^Invitation re-sent with a new link/),
    ).toBeDefined();
  });

  it("marks an invitation that has already expired", async () => {
    vi.mocked(listInvites).mockResolvedValue([
      invite({ expires_at: "2020-01-01T00:00:00Z" }),
    ]);
    renderAdmin();

    const invites = await sectionTable("Invitations");
    expect(within(invites).getByText(/^expired /).className).toContain(
      "text-state-failed",
    );

    // Resend stays available: it issues a new expiry.
    const resendButton: HTMLButtonElement = within(invites).getByRole(
      "button",
      { name: "Resend" },
    );
    expect(resendButton.disabled).toBe(false);
  });

  it("never shows an invitation token", async () => {
    renderAdmin();

    const invites = await sectionTable("Invitations");
    expect(invites.textContent?.toLowerCase()).not.toContain("token");
  });
});
