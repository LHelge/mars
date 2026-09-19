// Every TanStack Query key in the application, one group per resource. Add a
// group for a new resource rather than editing someone else's, so two pages
// being built in parallel never collide here.

import type { SessionState } from "../types";

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
  },

  users: {
    all: ["users"] as const,
    /** Admin only; the dashboard uses it to name a task's assignee. */
    list: () => ["users", "list"] as const,
  },
};
