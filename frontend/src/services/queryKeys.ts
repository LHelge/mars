// Every TanStack Query key in the application, one group per resource. Add a
// group for a new resource rather than editing someone else's, so two pages
// being built in parallel never collide here.

import type { SecretScope, SessionState } from "../types";

export const queryKeys = {
  sessions: {
    all: ["sessions"] as const,
    /** Without a state: every session the dashboard's list endpoint returns. */
    list: (state?: SessionState) => ["sessions", "list", state ?? "all"] as const,
  },

  tasks: {
    all: ["tasks"] as const,
    /** Tasks waiting in a human state, across all projects. */
    human: () => ["tasks", "human"] as const,
  },

  projects: {
    all: ["projects"] as const,
    list: () => ["projects", "list"] as const,
    /** One project, as the project page and every one of its tabs read it; the list warms it. */
    detail: (id: string) => ["projects", id] as const,
    /** The mirror's refs (`GET /projects/{id}/branches`). */
    branches: (id: string) => ["projects", id, "branches"] as const,
    sessions: (id: string) => ["projects", id, "sessions"] as const,
    /** The project's shared directories (`GET /projects/{id}/shared-dirs`). */
    sharedDirs: (id: string) => ["projects", id, "shared-dirs"] as const,
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
