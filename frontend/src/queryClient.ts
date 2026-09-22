// The one TanStack Query client for the application (`CLAUDE.md`, "Frontend
// conventions": server state through TanStack Query).
//
// Any answer below 500 is an answer, not a hiccup: a 401, 403 or 404, and
// equally a 400, 409, 422 or 429, says the same thing the second time. What is
// worth one more try is what never reached a verdict — a network failure, or a
// 5xx from an orchestrator that is restarting or a proxy answering 502 — so
// that one retry is left to the default and no call site turns it off.

import { QueryClient } from "@tanstack/react-query";
import { ApiError } from "./services/apiClient";

export function shouldRetry(failureCount: number, error: unknown): boolean {
  if (error instanceof ApiError && error.status < 500) {
    return false;
  }
  return failureCount < 1;
}

export function createQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: {
      queries: {
        retry: shouldRetry,
        staleTime: 5_000,
        refetchOnWindowFocus: true,
      },
    },
  });
}

export const queryClient = createQueryClient();
