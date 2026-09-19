// `SPEC.md`, "Frontend", Copy links: "preserve the internal destination
// through login and any required first-login password change, then open that
// task drawer or session. Only accept same-origin application paths as return
// destinations."
//
// The destination travels in router `state`, never in the URL, so a token or a
// search string never reaches the address bar.

import { useLocation } from "react-router";

/** Paths that would bounce the user straight back out of the application. */
const EXCLUDED_EXACT = ["/login", "/forgot-password"];
const EXCLUDED_PREFIXES = ["/invite/", "/reset-password/"];

/**
 * Returns `value` when it is a same-origin application path worth returning
 * to, and `null` otherwise: anything that is not a string, the empty string, a
 * protocol-relative path (`//evil.example`), an absolute URL with a scheme, a
 * backslash escape (`/\evil.example`, which browsers read as `//`), anything
 * carrying control characters or whitespace, and the unauthenticated routes
 * themselves.
 */
export function safeReturnTo(value: unknown): string | null {
  if (typeof value !== "string" || value.length === 0) {
    return null;
  }
  // A scheme ("https://x") can never appear once we require a leading slash.
  if (!value.startsWith("/")) {
    return null;
  }
  if (value.startsWith("//") || value.startsWith("/\\")) {
    return null;
  }
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f\s]/.test(value)) {
    return null;
  }

  const path = value.split(/[?#]/)[0];
  if (EXCLUDED_EXACT.includes(path)) {
    return null;
  }
  if (EXCLUDED_PREFIXES.some((prefix) => path.startsWith(prefix))) {
    return null;
  }

  return value;
}

/**
 * The destination a guard stashed in `location.state.from`, validated again on
 * the way out: router state survives history entries and is not trustworthy on
 * its own. Pages navigate to `useReturnTo() ?? "/"` after a successful sign-in
 * or password change.
 */
export function useReturnTo(): string | null {
  // Router state is untyped by construction; narrow it before use.
  const state: unknown = useLocation().state;
  if (typeof state !== "object" || state === null || !("from" in state)) {
    return null;
  }
  // `"from" in state` has already narrowed `state` to an object carrying it.
  return safeReturnTo(state.from);
}
