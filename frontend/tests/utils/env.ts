// The facts about the running end-to-end stack, and the two small utilities
// every other helper module leans on.
//
// `tests/e2e-stack.sh up` (`npm run test:e2e:up`) writes the connection facts
// to `frontend/.e2e/env`, which `playwright.config.ts` copies into
// `process.env` (README.md, "Development" → "End-to-end tests"). Each accessor
// is a function, not a constant, so a spec that needs no stack — `smoke.spec.ts`
// — never touches one, and a spec that does fails with the variable's name
// instead of a connection refused halfway through a scenario.

/** The variables `tests/e2e-stack.sh up` writes, and nothing else. */
type StackVariable =
  | "PLAYWRIGHT_BASE_URL"
  | "PLAYWRIGHT_API_URL"
  | "PLAYWRIGHT_ORCHESTRATOR_LOG"
  | "PLAYWRIGHT_DATA_DIR"
  | "PLAYWRIGHT_REPOS_DIR"
  | "PLAYWRIGHT_STUB_IMAGE"
  | "PLAYWRIGHT_ENGINE";

function requireEnv(name: StackVariable): string {
  const value = process.env[name];
  if (value === undefined || value === "") {
    throw new Error(
      `${name} is not set: this scenario needs the end-to-end stack. ` +
        "Start it with `npm run test:e2e:up`, which writes frontend/.e2e/env " +
        'for playwright.config.ts to load (README.md, "Development" → ' +
        '"End-to-end tests").',
    );
  }
  return value;
}

/** The origin the frontend under test is served from. */
export function baseUrl(): string {
  return requireEnv("PLAYWRIGHT_BASE_URL");
}

/** The orchestrator's API origin; REST paths hang under `<apiBaseUrl>/api`. */
export function apiBaseUrl(): string {
  return requireEnv("PLAYWRIGHT_API_URL");
}

/** The file the stack's orchestrator writes its `tracing` output to. */
export function orchestratorLogPath(): string {
  return requireEnv("PLAYWRIGHT_ORCHESTRATOR_LOG");
}

/** `DATA_DIR` of the running orchestrator (ARCHITECTURE.md, "Storage"). */
export function dataDir(): string {
  return requireEnv("PLAYWRIGHT_DATA_DIR");
}

/** The directory the local bare upstream repositories are created in. */
export function reposDir(): string {
  return requireEnv("PLAYWRIGHT_REPOS_DIR");
}

/** The stub session image the stack built and the profiles run. */
export function stubImage(): string {
  return requireEnv("PLAYWRIGHT_STUB_IMAGE");
}

/** The container engine binary the stack runs on: `podman` or `docker`. */
export function engineBinary(): string {
  return requireEnv("PLAYWRIGHT_ENGINE");
}

/** Eight hexadecimal characters; collision-free enough for one test run. */
export function randomSuffix(): string {
  let suffix = "";
  for (let i = 0; i < 8; i += 1) {
    suffix += Math.floor(Math.random() * 16).toString(16);
  }
  return suffix;
}

/** `<prefix>-<8 hex>`: a name no other test in the run can be holding. */
export function uniqueName(prefix: string): string {
  return `${prefix}-${randomSuffix()}`;
}

export interface WaitForOptions {
  timeoutMs?: number;
  intervalMs?: number;
  /** Named in the timeout error, so a failing report says what never happened. */
  description?: string;
}

/**
 * Polls `fn` until it returns something other than `null`/`undefined`/`false`
 * and returns that value. Nothing is logged on the way: a successful wait is
 * silent, and the timeout carries the description instead.
 */
export async function waitFor<T>(
  fn: () =>
    Promise<T | null | undefined | false> | T | null | undefined | false,
  opts: WaitForOptions = {},
): Promise<T> {
  const timeoutMs = opts.timeoutMs ?? 30_000;
  const intervalMs = opts.intervalMs ?? 250;
  const deadline = Date.now() + timeoutMs;

  for (;;) {
    const value = await fn();
    if (value !== null && value !== undefined && value !== false) {
      return value;
    }
    if (Date.now() >= deadline) {
      throw new Error(
        `timed out after ${timeoutMs} ms waiting for ${opts.description ?? "a condition"}`,
      );
    }
    await sleep(intervalMs);
  }
}

export function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
