import { expect, type APIRequestContext, type Page } from "@playwright/test";

/**
 * The orchestrator's API origin. The E2E epic runs a real orchestrator built
 * with `--features integration-tests`; API_PORT defaults to 7000
 * (README.md, "Configuration").
 */
export const apiBaseUrl =
  process.env.PLAYWRIGHT_API_URL ?? "http://localhost:7000";

export interface TestUser {
  username: string;
  email: string;
  password: string;
  access_token: string;
}

/** Response of `POST /api/test/users` (SPEC.md, "Test-only routes"). */
interface TestUserResponse {
  user: { id: string; username: string; email: string };
  access_token: string;
}

/**
 * Creates a fresh user through the test-only route and returns its credentials.
 * Obviously fake password and an `example.test` address: no real credentials
 * ever live in the repository (CLAUDE.md, mandatory rule 3).
 *
 * Requires an orchestrator built with the `integration-tests` feature;
 * exercised by the E2E epic.
 */
export async function createTestUser(
  request: APIRequestContext,
  opts?: { admin?: boolean },
): Promise<TestUser> {
  const suffix = Math.random().toString(36).slice(2, 10);
  const username = `e2e-${suffix}`;
  const email = `${username}@example.test`;
  const password = `e2e-not-a-real-password-${suffix}`;

  const response = await request.post(`${apiBaseUrl}/api/test/users`, {
    data: { username, email, password, admin: opts?.admin ?? false },
  });
  expect(response.status()).toBe(201);
  const body = (await response.json()) as TestUserResponse;

  return { username, email, password, access_token: body.access_token };
}

/**
 * Signs in through the login form and waits for the dashboard.
 *
 * implemented against SPEC.md "Frontend" routes; exercised by the E2E epic
 */
export async function login(
  page: Page,
  username: string,
  password: string,
): Promise<void> {
  await page.goto("/login");
  await page.getByLabel("Username").fill(username);
  await page.getByLabel("Password").fill(password);
  await page.getByRole("button", { name: "Sign in" }).click();
  await page.waitForURL("/");
}
