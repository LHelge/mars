// The one TanStack Query client for the application (`CLAUDE.md`, "Frontend
// conventions": server state through TanStack Query).
//
// A 401, 403 or 404 is an answer, not a hiccup: retrying it wastes a round trip
// and, for a 403, would fire the current-user refresh again. Everything else is
// retried once.

import { QueryClient } from "@tanstack/react-query";
import { ApiError } from "./services/apiClient";

const NEVER_RETRIED = [401, 403, 404];

export function shouldRetry(failureCount: number, error: unknown): boolean {
  if (error instanceof ApiError && NEVER_RETRIED.includes(error.status)) {
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
