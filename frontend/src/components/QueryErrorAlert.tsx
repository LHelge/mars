// A failed read, as one line with the button that tries it again.
//
// TanStack Query v5 leaves `data` in place through a failed refetch, so a view
// that already has something to show keeps showing it and puts this banner
// above it; the same component is the blocking state when the query has
// nothing (`SPEC.md`, "Frontend", Read failures). The sentence stays the
// caller's — every page words its own failures — so this holds the shape and
// the retry, never an error-to-text policy of its own.

import type { ReactNode } from "react";
import { Alert } from "./Alert";
import type { AlertKind } from "./Alert";
import { SubmitButton } from "./SubmitButton";

/** The little of a `UseQueryResult` the banner reads. */
export interface RetryableQuery {
  isFetching: boolean;
  refetch: () => Promise<unknown>;
}

export interface QueryErrorAlertProps {
  /** What this page says about the failure, already worded by the caller. */
  message: ReactNode;
  /**
   * The query that failed: its `refetch` is what the button does and its
   * `isFetching` the spinner. Leave it out, with no `onRetry`, for a refusal
   * that trying again cannot change.
   */
  query?: RetryableQuery;
  /** An action other than `query.refetch()` — a way out rather than a retry. */
  onRetry?: () => void;
  retryLabel?: string;
  /** `warning` when the failure only costs the view a detail. */
  kind?: AlertKind;
}

export function QueryErrorAlert({
  message,
  query,
  onRetry,
  retryLabel = "Try again",
  kind = "error",
}: QueryErrorAlertProps) {
  const retry =
    onRetry ??
    (query === undefined
      ? undefined
      : () => {
          void query.refetch();
        });

  return (
    <Alert kind={kind}>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span>{message}</span>
        {retry !== undefined && (
          <SubmitButton
            type="button"
            variant="ghost"
            loading={query?.isFetching ?? false}
            onClick={retry}
          >
            {retryLabel}
          </SubmitButton>
        )}
      </div>
    </Alert>
  );
}
