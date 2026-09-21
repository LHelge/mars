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

import type { Page } from "@playwright/test";

import type { SecretMeta } from "../src/types";
import { apiClient, expect, test } from "./utils/fixtures";
import {
  createTestUser,
  loginViaToken,
  newLoggedInPage,
  randomSuffix,
  setProfileSecrets,
  setProjectSecret,
  waitFor,
  waitForSessionState,
  type Api,
} from "./utils/test-helpers";

// Every other spec's user is given an agent credential by the `api` fixture, so
// the launch forms take their ordinary path. This file is the one that is
// *about* having none: the guided form creates the first one, and the launch
// scenario below starts from the warning.
test.use({ agentCredential: false });

/** Obviously fake, and the whole point of the scenarios below (rule 3). */
const VALUE = "fake-value-1";
const REPLACEMENT = "fake-value-2";
/** Agent credentials, equally fake; the names are the CLI's, the values are not. */
const FAKE_OAUTH_TOKEN = "fake-oauth-token-for-tests";
const FAKE_API_KEY = "fake-api-key-for-tests";

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
  user,
  api,
}) => {
  await loginViaToken(context, user);

  const name = secretName("API_TOKEN");
  // Not a prefix of the first name and not prefixed by it: the row locators
  // match on text, so one name must never be a substring of the other.
  const renamed = name.replace("API_TOKEN_", "API_TOKEN_2_");

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
    api,
    await readSecret(api, "?scope=global", name),
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
      const secret = await readSecret(api, "?scope=global", name);
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
    (await readSecret(api, "?scope=global", renamed)).orchestrator_only,
  ).toBe(true);

  // --- a duplicate name is refused ------------------------------------------

  await addSecret(page, renamed, VALUE);
  await expect(
    page.getByText("A secret with that name already exists in this scope."),
  ).toBeVisible();
  const duplicate = await api.send(
    "POST",
    "/secrets",
    { scope: "global", name: renamed, value: VALUE },
    { allow: [409] },
  );
  expect(duplicate.status).toBe(409);
  await expectValueNeverShown(page, VALUE);

  // --- delete ---------------------------------------------------------------

  await renamedRow.getByRole("button", { name: "Delete", exact: true }).click();
  // The confirmation opens under the row and names the secret it is about.
  await page
    .getByRole("button", { name: `Delete ${renamed}`, exact: true })
    .click();
  await expect(secretRow(page, renamed)).toHaveCount(0);

  const remaining = await api.get<SecretMeta[]>("/secrets?scope=global");
  expect(remaining.some((secret) => secret.name === renamed)).toBe(false);
});

test("project secrets are shared and user secrets are the owner's or an admin's", async ({
  page,
  context,
  request,
  browser,
  user: owner,
  api: ownerClient,
  project,
}) => {
  const other = await createTestUser(request, { prefix: "other" });
  const admin = await createTestUser(request, {
    prefix: "scope-admin",
    admin: true,
  });
  const otherClient = apiClient(request, other.access_token);
  const adminClient = apiClient(request, admin.access_token);

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
  user,
  api,
  project,
  sessions,
}) => {
  test.slow();

  const name = secretName("PROJECT_KEY");
  trackSecret(api, await setProjectSecret(api, project.id, name, VALUE));
  await setProfileSecrets(api, project.id, [name]);

  const session = await sessions.launch(api, project.id, {
    title: "secret uses",
  });
  await waitForSessionState(api, session.id, "running");
  // `end` stops the container and fetches the branch back; the audit row was
  // written at launch either way (`ARCHITECTURE.md`, "Launch sequence").
  await api.post(`/sessions/${session.id}/end`);
  await waitForSessionState(api, session.id, ["done", "failed"]);

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
    api,
    `?scope=project&scope_id=${project.id}`,
    name,
  );
  expect(meta.last_used_at).not.toBeNull();
});

test("an agent credential is added under its label, and a second kind at that scope is refused", async ({
  page,
  context,
  user,
  api,
}) => {
  await loginViaToken(context, user);

  await page.goto("/secrets");

  // Three fields and no name: the kind names the secret (`SPEC.md`,
  // "Frontend", Agent credentials).
  const form = page.getByRole("form", { name: "Add agent credential" });
  await expect(form).toBeVisible();
  await expect(form.getByLabel("Name")).toHaveCount(0);
  // The defaults are the subscription token and `Me`.
  await expect(
    form.getByRole("radio", { name: "Claude subscription token" }),
  ).toBeChecked();
  await expect(form.getByRole("radio", { name: "Me" })).toBeChecked();

  await form.getByLabel("Value").fill(FAKE_OAUTH_TOKEN);
  await form.getByRole("button", { name: "Add agent credential" }).click();

  const credential = secretRow(page, "Claude subscription token");
  await expect(credential).toBeVisible();
  await expect(credential).toContainText("You");
  await expectValueNeverShown(page, FAKE_OAUTH_TOKEN);

  // It is an ordinary secret under the name the table dictates, at the
  // caller's own user scope, and never orchestrator-only (ADR 0036).
  const created = trackSecret(
    api,
    await readSecret(api, "?scope=user", "CLAUDE_CODE_OAUTH_TOKEN"),
  );
  expect(created.scope).toBe("user");
  expect(created.scope_id).toBe(user.id);
  expect(created.credential_for).toBe("claude");
  expect(created.orchestrator_only).toBe(false);

  // The general list below is where it is *not*: neither its name nor its
  // label appears in the table of the scope that holds it.
  await page.getByRole("radio", { name: "My secrets" }).click();
  await expect(page.getByRole("heading", { name: "My secrets" })).toBeVisible();
  await expect(page.getByText("CLAUDE_CODE_OAUTH_TOKEN")).toHaveCount(0);
  await expect(secretRow(page, "Claude subscription token")).toHaveCount(1);

  // --- a second credential at the same scope --------------------------------

  await form.getByRole("radio", { name: "Anthropic API key" }).click();
  await form.getByLabel("Value").fill(FAKE_API_KEY);
  await form.getByRole("button", { name: "Add agent credential" }).click();

  // The body's `error` verbatim: it names the credential already there
  // (`SPEC.md`, "Secrets").
  await expect(
    page.getByText(
      "this scope already has an agent credential (CLAUDE_CODE_OAUTH_TOKEN); replace or delete it first",
    ),
  ).toBeVisible();
  await expectValueNeverShown(page, FAKE_API_KEY);

  const refused = await api.send(
    "POST",
    "/secrets",
    { scope: "user", name: "ANTHROPIC_API_KEY", value: FAKE_API_KEY },
    { allow: [409] },
  );
  expect(refused.status).toBe(409);

  // --- and deleting it takes the section back to its warning -----------------

  await secretRow(page, "Claude subscription token")
    .getByRole("button", { name: "Delete", exact: true })
    .click();
  await page
    .getByRole("button", {
      name: "Delete the Claude subscription token that applies to You",
      exact: true,
    })
    .click();
  await expect(secretRow(page, "Claude subscription token")).toHaveCount(0);
  await expect(
    page.getByText(/Sessions cannot authenticate without one/),
  ).toBeVisible();
});

test("the launch form warns without a credential, launches anyway, and names the credential once there is one", async ({
  page,
  context,
  user,
  api,
  project,
  sessions,
}) => {
  test.slow();

  await loginViaToken(context, user);

  // --- with no credential ---------------------------------------------------

  await page.goto(`/projects/${project.id}?tab=sessions`);
  const form = page.getByRole("form", { name: "Launch a session" });
  await expect(form).toBeVisible();

  await expect(
    form.getByText(
      "No agent credential: sessions of this profile will fail to authenticate",
    ),
  ).toBeVisible();
  await expect(
    form.getByRole("button", { name: "Add credential" }),
  ).toBeVisible();
  // The ordinary primary action is gone; the launch is the secondary one.
  await expect(
    form.getByRole("button", { name: "Launch session" }),
  ).toHaveCount(0);

  // --- `Launch anyway` still launches (ADR 0036) ----------------------------

  await form.getByLabel("First message (optional)").fill("launched anyway");
  await form.getByRole("button", { name: "Launch anyway" }).click();
  await page.waitForURL(/\/sessions\/[0-9a-f-]{8}-/);
  const sessionId = sessions.track(
    api,
    page.url().slice(page.url().lastIndexOf("/") + 1),
  );
  await waitForSessionState(api, sessionId, ["running", "done", "failed"]);

  // --- the warning's link is the way to fix it ------------------------------

  await page.goto(`/projects/${project.id}?tab=sessions`);
  await form.getByRole("button", { name: "Add credential" }).click();
  await page.waitForURL(/\/secrets$/);

  const credentialForm = page.getByRole("form", {
    name: "Add agent credential",
  });
  await credentialForm.getByLabel("Value").fill(FAKE_OAUTH_TOKEN);
  await credentialForm
    .getByRole("button", { name: "Add agent credential" })
    .click();
  await expect(secretRow(page, "Claude subscription token")).toBeVisible();
  trackSecret(api, await readSecret(api, "?scope=user", "CLAUDE_CODE_OAUTH_TOKEN"));

  // --- and the form says whose credential it is -----------------------------

  await page.goto(`/projects/${project.id}?tab=sessions`);
  await expect(
    form.getByText("Authenticates with your Claude subscription token"),
  ).toBeVisible();
  await expect(
    form.getByRole("button", { name: "Launch session" }),
  ).toBeVisible();
  await expect(
    form.getByRole("button", { name: "Add credential" }),
  ).toHaveCount(0);
});

test("a name that is not an environment-variable name is refused", async ({
  page,
  context,
  user,
  api,
}) => {
  await loginViaToken(context, user);

  // The API is the authority: `^[A-Z][A-Z0-9_]{0,127}$` (`docs/data-model.md`,
  // `secrets`), and anything else is a 400 whether or not a browser checked
  // first.
  for (const name of ["", "WITH SPACES"]) {
    const refused = await api.send(
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
  const globals = await api.get<SecretMeta[]>("/secrets?scope=global");
  expect(globals.some((secret) => secret.name.includes(" "))).toBe(false);
});
