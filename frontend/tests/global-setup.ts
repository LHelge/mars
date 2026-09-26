// Runs once before every `playwright test` (`playwright.config.ts`,
// `globalSetup`): clears the orchestrator's login throttle through the
// test-only route (`SPEC.md`, "Test-only routes").
//
// The suite's deliberate failed logins — a wrong password, a deleted user, the
// password a reset replaced, and on a rerun the seeded-administrator probe —
// all come from one client address, and the throttle blocks that address after
// ten failures in fifteen minutes. Starting every run from an empty throttle is
// what lets any number of runs share one `npm run test:e2e:up`
// (`tests/README.md`, "Running it").
//
// Without a stack there is nothing to reset: `smoke.spec.ts` runs without one,
// and every scenario that needs the stack fails on its own with the variable's
// name (`utils/env.ts`). With one, a refusal is fatal, because a run that
// cannot reset the throttle fails later with 429s that point nowhere near the
// cause.

export default async function globalSetup(): Promise<void> {
  const apiUrl = process.env.PLAYWRIGHT_API_URL;
  if (apiUrl === undefined || apiUrl === "") return;

  const response = await fetch(`${apiUrl}/api/test/throttle/reset`, {
    method: "POST",
  });
  if (response.status !== 204) {
    throw new Error(
      `POST ${apiUrl}/api/test/throttle/reset answered ${response.status}: ` +
        "the stack's orchestrator must be built with --features integration-tests " +
        '(README.md, "Development" → "End-to-end tests")',
    );
  }
}
