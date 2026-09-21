// Loading and error state for every form in the app (`CLAUDE.md`, "Frontend
// conventions": "`useFormSubmit()` for form loading and error state").
//
// Generic over the action's arguments, so a page can wire it to either
// `submit(event)` or `submit(values)`.

import { useCallback, useEffect, useRef, useState } from "react";
import { errorMessage, logUnexpected } from "../services/errorMessage";

export interface UseFormSubmit<TArgs extends unknown[]> {
  submit: (...args: TArgs) => Promise<void>;
  loading: boolean;
  error: string | null;
  clearError: () => void;
}

export interface UseFormSubmitOptions {
  /**
   * This form's wording for a failure, in place of `errorMessage`'s. It is
   * how a page says "Invalid username or password" for a 401 without forging
   * an `ApiError` to smuggle the sentence through; it delegates whatever it
   * has no opinion about back to `errorMessage`.
   */
  mapError?: (caught: unknown) => string;
}

export function useFormSubmit<TArgs extends unknown[] = []>(
  action: (...args: TArgs) => Promise<void>,
  options?: UseFormSubmitOptions,
): UseFormSubmit<TArgs> {
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // A ref, not `loading`: a double-click dispatches both handlers before React
  // has re-rendered with the new state.
  const inFlight = useRef(false);

  // Kept in a ref so `submit` stays stable across renders even when the page
  // passes an inline closure. Written after commit, never during render.
  const actionRef = useRef(action);
  const mapErrorRef = useRef(options?.mapError);
  useEffect(() => {
    actionRef.current = action;
    mapErrorRef.current = options?.mapError;
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
      // The one place this failure is logged: a formatter called from render
      // would log again on every keystroke while the alert is up.
      logUnexpected(caught);
      const map = mapErrorRef.current;
      setError(map === undefined ? errorMessage(caught) : map(caught));
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
