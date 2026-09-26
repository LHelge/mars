/**
 * How many events one page of session history asks for (`SPEC.md`, "Frontend",
 * Session state): the first page a view opens with and every older page after
 * it.
 *
 * A build-time constant, 200 unless the build is given
 * `VITE_SESSION_HISTORY_PAGE_SIZE`. Only the end-to-end dev server is
 * (`playwright.config.ts`): the scenario that reads older history has to fill
 * more than one page first, and a smaller page is fewer inputs to fill it
 * with (`tests/README.md`, "Wall-clock"). The window is the client's choice —
 * `GET /sessions/{id}/events` takes any `limit` up to its cap — so nothing on
 * the server has to agree with it.
 */
export const DEFAULT_HISTORY_PAGE_SIZE = 200;

/** `GET /sessions/{id}/events` refuses a `limit` above this. */
export const MAX_HISTORY_PAGE_SIZE = 500;

/**
 * The page size a build-time value asks for, or the default when it is unset
 * or is not a whole number from 1 to the endpoint's cap: a typo in a test
 * knob must not turn into a request the endpoint refuses.
 */
export function parseHistoryPageSize(raw: string | undefined): number {
  if (raw === undefined || !/^\d+$/.test(raw.trim())) {
    return DEFAULT_HISTORY_PAGE_SIZE;
  }
  const size = Number(raw.trim());
  return size >= 1 && size <= MAX_HISTORY_PAGE_SIZE
    ? size
    : DEFAULT_HISTORY_PAGE_SIZE;
}

export const HISTORY_PAGE_SIZE = parseHistoryPageSize(
  import.meta.env.VITE_SESSION_HISTORY_PAGE_SIZE as string | undefined,
);
