// The helper layer asserting itself against the real stack.
//
// Every other scenario is written in terms of `tests/utils/test-helpers.ts`,
// so this spec is the one that fails first when a helper stops matching the
// orchestrator: a user, a seeded browser, a local upstream, a project that
// finished cloning, and a link read back out of the orchestrator's log.
//
// Unlike `smoke.spec.ts` this needs `npm run test:e2e:up`; without it the
// first helper throws the accessor's error naming the missing variable.

import { expect, test } from "@playwright/test";

import type { Invite, InviteLookup } from "../src/types";
import {
  api,
  createBareRepo,
  createProject,
  createTestUser,
  currentUser,
  gitRevParse,
  listBranches,
  logOffset,
  loginViaToken,
  readLoggedLink,
} from "./utils/test-helpers";

test("a fresh user is signed in and answers GET /users/me", async ({
  request,
}) => {
  const user = await createTestUser(request, { prefix: "me" });
  const client = api(request, user.access_token);

  const me = await currentUser(client);

  expect(me.id).toBe(user.id);
  expect(me.username).toBe(user.username);
  expect(me.email).toBe(user.email);
  expect(me.admin).toBe(false);
  expect(user.refresh_cookie).not.toBe("");
});

test("an unknown path throws with its status and body", async ({ request }) => {
  const user = await createTestUser(request, { prefix: "err" });
  const client = api(request, user.access_token);

  await expect(client.get("/projects/not-a-uuid")).rejects.toThrow(
    /GET \/projects\/not-a-uuid → 4\d\d/,
  );
});

test("a token-seeded browser lands on the dashboard", async ({
  page,
  context,
  request,
}) => {
  const user = await createTestUser(request, { prefix: "seed" });
  await loginViaToken(context, user);

  await page.goto("/");

  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByRole("heading", { name: "Dashboard" })).toBeVisible();
});

test("a bare repository is created at its initial commit and clones into a project", async ({
  request,
}) => {
  const user = await createTestUser(request, { prefix: "repo", admin: true });
  const client = api(request, user.access_token);

  const repo = createBareRepo("fixture", {
    files: { "src/main.rs": "fn main() {}\n" },
  });
  expect(gitRevParse(repo.path, "main")).toBe(repo.initialCommit);
  expect(repo.url.startsWith("file:///")).toBe(true);

  const project = await createProject(client, { remote_url: repo.url });
  expect(project.status).toBe("ready");
  expect(project.default_branch).toBe("main");

  const branches = await listBranches(client, project.id);
  const names = branches.map((branch) => branch.name);
  expect(names).toContain("main");
  expect(names).toContain("origin/main");
});

test("an invitation's link is read back out of the orchestrator log", async ({
  request,
}) => {
  const admin = await createTestUser(request, { prefix: "admin", admin: true });
  const client = api(request, admin.access_token);
  const invited = `e2e-invited-${Date.now()}@example.test`;

  const offset = logOffset();
  const invite = await client.post<Invite>("/users/invites", {
    email: invited,
  });
  expect(invite.email).toBe(invited);

  const link = await readLoggedLink("invite", invited, offset);

  expect(link).toMatch(/\/invite\/[A-Za-z0-9._~-]+$/);
  const token = link.slice(link.lastIndexOf("/") + 1);
  const lookup = await client.get<InviteLookup>(`/auth/invite/${token}`);
  expect(lookup.email).toBe(invited);
});
