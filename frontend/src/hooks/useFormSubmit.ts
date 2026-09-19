// Loading and error state for every form in the app (`CLAUDE.md`, "Frontend
// conventions": "`useFormSubmit()` for form loading and error state").
//
// Generic over the action's arguments, so a page can wire it to either
// `submit(event)` or `submit(values)`.

import { useCallback, useEffect, useRef, useState } from "react";
import { ApiError } from "../services/apiClient";

/** What the user sees when the failure is not an `{ status, error }` body. */
const FALLBACK_ERROR = "Something went wrong";

export interface UseFormSubmit<TArgs extends unknown[]> {
  submit: (...args: TArgs) => Promise<void>;
  loading: boolean;
  error: string | null;
  clearError: () => void;
}

export function useFormSubmit<TArgs extends unknown[] = []>(
  action: (...args: TArgs) => Promise<void>,
): UseFormSubmit<TArgs> {
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // A ref, not `loading`: a double-click dispatches both handlers before React
  // has re-rendered with the new state.
  const inFlight = useRef(false);

  // Kept in a ref so `submit` stays stable across renders even when the page
  // passes an inline closure. Written after commit, never during render.
  const actionRef = useRef(action);
  useEffect(() => {
    actionRef.current = action;
  });

  const submit = useCallback(async (...args: TArgs): Promise<void> => {
    if (inFlight.current) {
      return;
    }
    inFlight.current = true;
    setLoading(true);
    setError(null);
    try {
      await actionRef.current(...args);
    } catch (caught) {
      if (caught instanceof ApiError) {
        setError(caught.error);
      } else {
        // Network failures and genuine bugs: say the same thing to the user
        // and keep the detail in the console.
        console.error(caught);
        setError(FALLBACK_ERROR);
      }
    } finally {
      inFlight.current = false;
      setLoading(false);
    }
  }, []);

  const clearError = useCallback(() => {
    setError(null);
  }, []);

  return { submit, loading, error, clearError };
}
