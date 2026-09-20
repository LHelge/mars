// The arrangement every scenario starts from, as Playwright fixtures.
//
// Almost every spec here needs the same four things: a fresh user, a REST
// client authenticated as that user, a local bare upstream, and a project that
// finished cloning from it. Each spec used to build them in a `stage()` of its
// own and end its sessions in an `afterEach` of its own; they are one module
// now, so the arrangement is described once and a scenario reads as the thing
// it is about.
//
// Fixtures are lazy: `user` is created only for a test that names it, and a
// spec that needs no stack at all — `smoke.spec.ts` — touches none of them.
// That laziness is why `repo` and `project` can live beside `user` without
// making a login scenario clone anything.
//
// **Importing.** A spec imports `test` and `expect` from here instead of from
// `@playwright/test`; everything else still comes from `./test-helpers`. The
// `api` fixture shadows the `api()` factory inside a test body, so a scenario
// that needs a *second* client — an admin, a reviewer, another browser — uses
// `apiClient(request, token)`, re-exported below under that name.
//
// **Per-spec repositories.** The upstream's extra files are an option fixture:
// a spec whose assertions name a file declares it once at the top with
// `test.use({ repoFiles: { "src/app.txt": "v1\n" } })` and every scenario in
// the file clones a repository that has it.

import { test as base } from "@playwright/test";
import type { TestInfo } from "@playwright/test";

import type { Project, Session } from "../../src/types";
import { api as apiClient } from "./api";
import { createTestUser } from "./api";
import type { Api, TestUser } from "./api";
import { uniqueName } from "./env";
import { createBareRepo } from "./git";
import type { BareRepo } from "./git";
import { createProject, endSession, launchSession } from "./resources";
import type { LaunchSessionOptions } from "./resources";

export { apiClient };

/**
 * Sessions the scenario launched, ended when it finishes.
 *
 * A container left running outlives the test and only `tests/e2e-stack.sh
 * down` would remove it, so every launch goes through here — including one
 * made from the UI, whose id the spec learns from the URL and hands to
 * [`SessionTracker.track`].
 */
export interface SessionTracker {
  /** [`launchSession`], registered for clean-up. */
  launch(
    client: Api,
    projectId: string,
    opts?: LaunchSessionOptions,
  ): Promise<Session>;
  /** Registers a session launched some other way, by id. */
  track(client: Api, sessionId: string): string;
}

export interface E2EOptions {
  /**
   * Extra files in the fixture upstream's initial commit, on top of the
   * `README.md` every one of them carries. Set per spec file with `test.use`.
   */
  repoFiles: Record<string, string>;
}

export interface E2EFixtures {
  /** A fresh user, created through the test-only route. */
  user: TestUser;
  /** A REST client authenticated as [`user`]. */
  api: Api;
  /** A local bare upstream with one commit on `main`. */
  repo: BareRepo;
  /** A project cloned from [`repo`], waited to `ready`. */
  project: Project;
  /** The scenario's sessions, ended when it finishes. */
  sessions: SessionTracker;
}

/**
 * The spec file's own name — `sessions`, `task-sessions`, `git` — used as the
 * prefix of the user name, the repository directory and the project name, so a
 * leftover row or a line in the orchestrator log says which file made it.
 */
function specPrefix(testInfo: TestInfo): string {
  const base = testInfo.file.slice(testInfo.file.lastIndexOf("/") + 1);
  return base.replace(/\.spec\.ts$/, "");
}

export const test = base.extend<E2EOptions & E2EFixtures>({
  repoFiles: [{}, { option: true }],

  user: async ({ request }, use, testInfo) => {
    await use(await createTestUser(request, { prefix: specPrefix(testInfo) }));
  },

  api: async ({ request, user }, use) => {
    await use(apiClient(request, user.access_token));
  },

  repo: async ({ repoFiles }, use, testInfo) => {
    await use(createBareRepo(specPrefix(testInfo), { files: repoFiles }));
  },

  project: async ({ api, repo }, use, testInfo) => {
    await use(
      await createProject(api, {
        name: uniqueName(specPrefix(testInfo)),
        remote_url: repo.url,
      }),
    );
  },

  sessions: async ({}, use) => {
    const launched: { client: Api; id: string }[] = [];
    const tracker: SessionTracker = {
      async launch(client, projectId, opts = {}) {
        const session = await launchSession(client, projectId, opts);
        launched.push({ client, id: session.id });
        return session;
      },
      track(client, sessionId) {
        launched.push({ client, id: sessionId });
        return sessionId;
      },
    };

    await use(tracker);

    for (const session of launched) {
      // Best effort: a scenario that already ended its session is fine, and one
      // that failed must still not leave a container behind.
      await endSession(session.client, session.id).catch(() => undefined);
    }
  },
});

export { expect } from "@playwright/test";
