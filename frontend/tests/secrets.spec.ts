// The write-only secrets manager end to end (`SPEC.md`, "User-facing
// features", Secrets, and "Secrets (`/api/secrets`)"): a round trip at global
// scope, the project and user scopes and who may see them, the uses trail a
// launch writes, and the name rule.
//
// The one invariant every scenario re-asserts is the write-only one: a value
// is typed once and never comes back. `expectValueNeverShown` scans the page
// text, every control's live `value` and every `value` attribute, because a
// leak could hide in any of the three. Its assertions are booleans rather than
// text comparisons so a failing run says *that* a value was on screen without
// printing it again into the report (`CLAUDE.md`, rule 3; the values here are
// obviously fake regardless).
//
// Global secrets are visible to every user of the stack, and the other spec
// files run against the same orchestrator, so nothing asserts on counts: every
// scenario looks for its own uniquely named rows and deletes what it created.

import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

import type { SecretMeta } from "../src/types";
import {
  api,
  createBareRepo,
  createProject,
  createTestUser,
  launchSession,
  loginViaToken,
  newLoggedInPage,
  randomSuffix,
  setProfileSecrets,
  setProjectSecret,
  uniqueName,
  waitFor,
  waitForSessionState,
  type Api,
} from "./utils/test-helpers";

/** Obviously fake, and the whole point of the scenarios below (rule 3). */
const VALUE = "fake-value-1";
const REPLACEMENT = "fake-value-2";

/**
 * A secret name no other test in the run holds. `uniqueName` joins with a
 * hyphen, which `^[A-Z][A-Z0-9_]{0,127}$` (`docs/data-model.md`, `secrets`)
 * does not allow, so the suffix is joined with an underscore instead.
 */
function secretName(prefix: string): string {
  return `${prefix}_${randomSuffix().toUpperCase()}`;
}

/** Secrets to remove after the scenario, whichever way it ended. */
const created: { client: Api; id: string }[] = [];

function trackSecret(client: Api, secret: SecretMeta): SecretMeta {
  created.push({ client, id: secret.id });
  return secret;
}

test.afterEach(async () => {
  const pending = created.splice(0, created.length);
  for (const entry of pending) {
    // A scenario that already deleted its secret leaves a 404 behind; that is
    // the clean-up succeeding, not failing.
    await entry.client.delete(`/secrets/${entry.id}`, undefined, {
      allow: [403, 404],
    });
  }
});

/**
 * Asserts that `value` is nowhere in the rendered page: not in its text, not
 * in a control's current value, not in a `value` attribute (which is where a
 * server-rendered `type="password"` field would carry it).
 */
async function expectValueNeverShown(page: Page, value: string): Promise<void> {
  const text = await page.locator("body").innerText();
  expect(text.includes(value), "the secret value is in the page text").toBe(
    false,
  );

  const fields = await page.locator("input, textarea").evaluateAll((nodes) =>
    nodes.map((node) => {
      const control = node as HTMLInputElement | HTMLTextAreaElement;
      return `${control.value}\u0000${control.getAttribute("value") ?? ""}`;
    }),
  );
  expect(
    fields.some((field) => field.includes(value)),
    "the secret value is in a form control",
  ).toBe(false);
}

/** The create form of the manager on screen, whichever page mounts it. */
async function addSecret(
  page: Page,
  name: string,
  value: string,
): Promise<void> {
  const form = page.getByRole("form", { name: "Add a secret" });
  await form.getByLabel("Name").fill(name);
  await form.getByLabel("Value").fill(value);
  await form.getByRole("button", { name: "Add secret" }).click();
}

/** The table row of one secret, by its name. */
function secretRow(page: Page, name: string) {
  return page.getByRole("row").filter({ hasText: name });
}

/** The one secret of a scope, read back through the API for its metadata. */
async function readSecret(
  client: Api,
  query: string,
  name: string,
): Promise<SecretMeta> {
  const secrets = await client.get<SecretMeta[]>(`/secrets${query}`);
  const secret = secrets.find((candidate) => candidate.name === name);
  if (secret === undefined) {
    throw new Error(`no secret named ${name} in GET /secrets${query}`);
  }
  return secret;
}

test("a global secret is created, replaced, renamed, flagged and deleted without ever showing its value", async ({
  page,
  context,
  request,
}) => {
  const user = await createTestUser(request, { prefix: "secret" });
  const client = api(request, user.access_token);
  await loginViaToken(context, user);

  const name = secretName("API_TOKEN");
  // Not a prefix of the first name and not prefixed by it: the row locators
  // match on text, so one name must never be a substring of the other.
  const renamed = name.replace("API_TOKEN_", "API_TOKEN_2_");

  // Every `window.confirm` in the manager is the delete confirmation.
  page.on("dialog", (dialog) => {
    void dialog.accept();
  });

  await page.goto("/secrets");
  await expect(
    page.getByRole("heading", { name: "Global secrets" }),
  ).toBeVisible();

  await addSecret(page, name, VALUE);

  const row = secretRow(page, name);
  await expect(row).toBeVisible();
  await expect(row).toContainText("v1");
  await expectValueNeverShown(page, VALUE);

  const afterCreate = trackSecret(
    client,
    await readSecret(client, "?scope=global", name),
  );
  expect(afterCreate.scope).toBe("global");
  expect(afterCreate.scope_id).toBeNull();
  expect(afterCreate.key_version).toBe(1);
  expect(afterCreate.created_by).toBe(user.id);
  expect(Date.parse(afterCreate.created_at)).not.toBeNaN();
  expect(Date.parse(afterCreate.updated_at)).not.toBeNaN();
  expect(afterCreate.last_used_at).toBeNull();

  // A reload is the second half of the write-only assertion: nothing that was
  // typed survives in the query cache or comes back from the server.
  await page.reload();
  await expect(secretRow(page, name)).toBeVisible();
  await expectValueNeverShown(page, VALUE);

  // --- replace the value ----------------------------------------------------

  await row.getByRole("button", { name: "Replace value" }).click();
  const replaceForm = page.getByRole("form", {
    name: `Replace the value of ${name}`,
  });
  await replaceForm.getByRole("textbox").fill(REPLACEMENT);
  await replaceForm.getByRole("button", { name: "Save value" }).click();
  await expect(replaceForm).toHaveCount(0);

  const afterReplace = await waitFor(
    async () => {
      const secret = await readSecret(client, "?scope=global", name);
      return secret.updated_at === afterCreate.updated_at ? null : secret;
    },
    { description: `${name} to record a new updated_at` },
  );
  expect(Date.parse(afterReplace.updated_at)).toBeGreaterThan(
    Date.parse(afterCreate.updated_at) - 1,
  );
  expect(afterReplace.key_version).toBe(1);
  await expectValueNeverShown(page, REPLACEMENT);

  // --- rename ---------------------------------------------------------------

  await row.getByRole("button", { name: "Rename" }).click();
  const renameForm = page.getByRole("form", { name: `Rename ${name}` });
  await renameForm.getByRole("textbox").fill(renamed);
  await renameForm.getByRole("button", { name: "Save name" }).click();

  await expect(secretRow(page, renamed)).toBeVisible();
  await expect(secretRow(page, name)).toHaveCount(0);

  // --- orchestrator only ----------------------------------------------------

  const renamedRow = secretRow(page, renamed);
  const flag = renamedRow.getByLabel(`Orchestrator only: ${renamed}`);
  await expect(flag).not.toBeChecked();
  // Controlled by the row's metadata: the click sends the `PATCH` and the
  // checkbox follows the answer.
  await flag.click();
  await expect(flag).toBeChecked();
  // The lock badge beside the checkbox is the row's visual mark.
  await expect(
    renamedRow.locator(
      'label[title="Never injected into session containers"] svg',
    ),
  ).toBeVisible();
  expect(
    (await readSecret(client, "?scope=global", renamed)).orchestrator_only,
  ).toBe(true);

  // --- a duplicate name is refused ------------------------------------------

  await addSecret(page, renamed, VALUE);
  await expect(
    page.getByText("A secret with that name already exists in this scope."),
  ).toBeVisible();
  const duplicate = await client.send(
    "POST",
    "/secrets",
    { scope: "global", name: renamed, value: VALUE },
    { allow: [409] },
  );
  expect(duplicate.status).toBe(409);
  await expectValueNeverShown(page, VALUE);

  // --- delete ---------------------------------------------------------------

  await renamedRow.getByRole("button", { name: "Delete" }).click();
  await expect(secretRow(page, renamed)).toHaveCount(0);

  const remaining = await client.get<SecretMeta[]>("/secrets?scope=global");
  expect(remaining.some((secret) => secret.name === renamed)).toBe(false);
});

test("project secrets are shared and user secrets are the owner's or an admin's", async ({
  page,
  context,
  request,
  browser,
}) => {
  const owner = await createTestUser(request, { prefix: "owner" });
  const other = await createTestUser(request, { prefix: "other" });
  const admin = await createTestUser(request, {
    prefix: "scope-admin",
    admin: true,
  });
  const ownerClient = api(request, owner.access_token);
  const otherClient = api(request, other.access_token);
  const adminClient = api(request, admin.access_token);

  const repo = createBareRepo("secret-scopes");
  const project = await createProject(ownerClient, {
    name: uniqueName("e2e-secret-scopes"),
    remote_url: repo.url,
  });
  const projectKey = secretName("PROJECT_KEY");
  const myKey = secretName("MY_KEY");

  await loginViaToken(context, owner);

  // --- the project's secrets tab --------------------------------------------

  await page.goto(`/projects/${project.id}?tab=secrets`);
  await expect(
    page.getByRole("heading", { name: "Project secrets" }),
  ).toBeVisible();
  await addSecret(page, projectKey, VALUE);
  await expect(secretRow(page, projectKey)).toBeVisible();
  await expectValueNeverShown(page, VALUE);

  trackSecret(
    ownerClient,
    await readSecret(
      ownerClient,
      `?scope=project&scope_id=${project.id}`,
      projectKey,
    ),
  );

  // --- the caller's own user scope ------------------------------------------

  await page.goto("/secrets");
  await page.getByRole("radio", { name: "My secrets" }).click();
  await expect(page.getByRole("heading", { name: "My secrets" })).toBeVisible();
  await addSecret(page, myKey, VALUE);
  await expect(secretRow(page, myKey)).toBeVisible();
  await expectValueNeverShown(page, VALUE);

  const mine = trackSecret(
    ownerClient,
    await readSecret(ownerClient, "?scope=user", myKey),
  );
  expect(mine.scope).toBe("user");
  expect(mine.scope_id).toBe(owner.id);

  // --- a second user --------------------------------------------------------

  const otherPage = await newLoggedInPage(browser, other);
  try {
    await otherPage.goto("/secrets?scope=user");
    await expect(
      otherPage.getByRole("heading", { name: "My secrets" }),
    ).toBeVisible();
    // Their own user scope is their own: the owner's secret is not in it.
    await expect(
      otherPage.getByRole("form", { name: "Add a secret" }),
    ).toBeVisible();
    await expect(secretRow(otherPage, myKey)).toHaveCount(0);

    // The project's secrets, though, belong to the project.
    await otherPage.goto(`/projects/${project.id}?tab=secrets`);
    await expect(secretRow(otherPage, projectKey)).toBeVisible();
    await expectValueNeverShown(otherPage, VALUE);

    // And the API refuses the owner's user scope outright (`SPEC.md`,
    // "Secrets": owner or admin, 403 otherwise).
    const forbidden = await otherClient.send(
      "GET",
      `/secrets?scope=user&scope_id=${owner.id}`,
      undefined,
      { allow: [403] },
    );
    expect(forbidden.status).toBe(403);
  } finally {
    // The page first: closing the context alone can wait on a page that
    // still has work in flight.
    await otherPage.close();
    await otherPage.context().close();
  }

  // --- an administrator picking that user -----------------------------------

  const adminPage = await newLoggedInPage(browser, admin);
  try {
    await adminPage.goto("/secrets");
    await adminPage.getByRole("radio", { name: "Another user" }).click();
    // Exactly "User": "Another user" is the radio beside this select.
    await adminPage.getByLabel("User", { exact: true }).selectOption(owner.id);
    await expect(
      adminPage.getByRole("heading", {
        name: `User secrets: ${owner.username}`,
      }),
    ).toBeVisible();
    await expect(secretRow(adminPage, myKey)).toBeVisible();
    await expectValueNeverShown(adminPage, VALUE);
  } finally {
    // The page first: closing the context alone can wait on a page that
    // still has work in flight.
    await adminPage.close();
    await adminPage.context().close();
  }

  expect(
    (
      await adminClient.get<SecretMeta[]>(
        `/secrets?scope=user&scope_id=${owner.id}`,
      )
    ).some((secret) => secret.name === myKey),
  ).toBe(true);
});

test("a launch writes a use, and the uses view lists it without the value", async ({
  page,
  context,
  request,
}) => {
  test.slow();

  const user = await createTestUser(request, { prefix: "uses" });
  const client = api(request, user.access_token);

  const repo = createBareRepo("secret-uses");
  const project = await createProject(client, {
    name: uniqueName("e2e-secret-uses"),
    remote_url: repo.url,
  });

  const name = secretName("PROJECT_KEY");
  trackSecret(client, await setProjectSecret(client, project.id, name, VALUE));
  await setProfileSecrets(client, project.id, [name]);

  const session = await launchSession(client, project.id, {
    title: "secret uses",
  });
  await waitForSessionState(client, session.id, "running");
  // `end` stops the container and fetches the branch back; the audit row was
  // written at launch either way (`ARCHITECTURE.md`, "Launch sequence").
  await client.post(`/sessions/${session.id}/end`);
  await waitForSessionState(client, session.id, ["done", "failed"]);

  await loginViaToken(context, user);
  await page.goto(`/projects/${project.id}?tab=secrets`);

  const row = secretRow(page, name);
  await expect(row).toBeVisible();
  // "never" is the placeholder of an unused secret; this one has been read.
  await expect(row).not.toContainText("never");

  await row.getByRole("button", { name: "Uses" }).click();
  const uses = page.getByRole("listitem").filter({ hasText: "launch" });
  await expect(uses).toHaveCount(1);
  await expect(uses).toContainText(session.id.slice(0, 8));

  await expectValueNeverShown(page, VALUE);

  const meta = await readSecret(
    client,
    `?scope=project&scope_id=${project.id}`,
    name,
  );
  expect(meta.last_used_at).not.toBeNull();
});

test("a name that is not an environment-variable name is refused", async ({
  page,
  context,
  request,
}) => {
  const user = await createTestUser(request, { prefix: "secret-name" });
  const client = api(request, user.access_token);
  await loginViaToken(context, user);

  // The API is the authority: `^[A-Z][A-Z0-9_]{0,127}$` (`docs/data-model.md`,
  // `secrets`), and anything else is a 400 whether or not a browser checked
  // first.
  for (const name of ["", "WITH SPACES"]) {
    const refused = await client.send(
      "POST",
      "/secrets",
      { scope: "global", name, value: VALUE },
      { allow: [400] },
    );
    expect(refused.status).toBe(400);
  }

  // The form never lets either one reach the orchestrator: an empty name is
  // the field's own `required`, and a name with spaces is caught by
  // `utils/secretName.ts`, which states the rule the server enforces. The
  // scenario asserts the message rather than the 400 the API answers above.
  await page.goto("/secrets");
  const form = page.getByRole("form", { name: "Add a secret" });
  await form.getByLabel("Value").fill(VALUE);
  await form.getByRole("button", { name: "Add secret" }).click();
  const missing = await form
    .getByLabel("Name")
    .evaluate((node) => (node as HTMLInputElement).validity.valueMissing);
  expect(missing).toBe(true);

  await form.getByLabel("Name").fill("WITH SPACES");
  await form.getByRole("button", { name: "Add secret" }).click();
  await expect(
    form.getByText(
      "Name must be an environment-variable name: uppercase letters, digits and underscores, starting with a letter",
    ),
  ).toBeVisible();

  // Nothing was created under either spelling.
  const globals = await client.get<SecretMeta[]>("/secrets?scope=global");
  expect(globals.some((secret) => secret.name.includes(" "))).toBe(false);
});
