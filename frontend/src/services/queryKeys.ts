// Every TanStack Query key in the application, one group per resource. Add a
// group for a new resource rather than editing someone else's, so two pages
// being built in parallel never collide here.

import type { SecretScope, SessionState } from "../types";

export const queryKeys = {
  sessions: {
    all: ["sessions"] as const,
    /** Without a state: every session the dashboard's list endpoint returns. */
    list: (state?: SessionState) =>
      ["sessions", "list", state ?? "all"] as const,
    /** One session, as the session page reads it and the socket keeps it current. */
    detail: (id: string) => ["sessions", id] as const,
    /** The tasks one session touched (`GET /sessions/{id}/tasks`). */
    tasks: (id: string) => ["sessions", id, "tasks"] as const,
  },

  tasks: {
    all: ["tasks"] as const,
    /** Tasks waiting in a human state, across all projects. */
    human: () => ["tasks", "human"] as const,
    /** One task with its comments and hand-offs, by UUID or per-project number. */
    detail: (projectId: string, idOrNumber: string | number) =>
      ["tasks", projectId, String(idOrNumber)] as const,
  },

  projects: {
    all: ["projects"] as const,
    list: () => ["projects", "list"] as const,
    /** One project, as the project page and every one of its tabs read it; the list warms it. */
    detail: (id: string) => ["projects", id] as const,
    /** The mirror's refs (`GET /projects/{id}/branches`). */
    branches: (id: string) => ["projects", id, "branches"] as const,
    /** Session refs with ahead/behind (`GET /projects/{id}/git/session-branches`). */
    sessionBranches: (id: string) =>
      ["projects", id, "git", "session-branches"] as const,
    /**
     * One diff (`GET /projects/{id}/git/diff`), as the Changes panel reads it.
     * Without a `base` the server compares against the project's default
     * branch, which is a key of its own: the answer is not the same query.
     */
    diff: (id: string, head: string, base?: string) =>
      ["projects", id, "git", "diff", { head, base: base ?? null }] as const,
    /**
     * One project's sessions. A state filter extends the key with a further
     * element, so invalidating this base key covers every filter at once.
     */
    sessions: (id: string) => ["projects", id, "sessions"] as const,
    /** The project's shared directories (`GET /projects/{id}/shared-dirs`). */
    sharedDirs: (id: string) => ["projects", id, "shared-dirs"] as const,
    /** The project's agent profiles (`GET /projects/{id}/profiles`). */
    profiles: (id: string) => ["projects", id, "profiles"] as const,
    /** The project's board columns (`GET /projects/{id}/task-states`). */
    taskStates: (id: string) => ["projects", id, "task-states"] as const,
  },

  secrets: {
    all: ["secrets"] as const,
    /**
     * One list per scope. `user` without a `scopeId` is "my secrets"; the page
     * normalises the caller's own id away so both spellings share one entry.
     */
    list: (scope: SecretScope, scopeId?: string | null) =>
      ["secrets", "list", scope, scopeId ?? "self"] as const,
    /** The audit trail of one secret, at the limit currently asked for. */
    uses: (id: string, limit = 20) => ["secrets", "uses", id, limit] as const,
  },

  users: {
    all: ["users"] as const,
    /** Admin only; the dashboard uses it to name a task's assignee. */
    list: () => ["users", "list"] as const,
    /** The signed-in user; refreshed in place after a self-service password change. */
    me: () => ["users", "me"] as const,
  },

  invites: {
    all: ["invites"] as const,
    /** Admin only; the open invitations of the administration page. */
    list: () => ["invites", "list"] as const,
  },
};
