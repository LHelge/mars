// Display formatting shared by every list and detail view. All three helpers
// take a value that may be missing and answer an em dash for it, so a caller
// never has to branch before calling them.

/** What a missing or unreadable value looks like everywhere in the UI. */
export const PLACEHOLDER = "—";

const MINUTE = 60;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

function parse(iso: string | null | undefined): Date | null {
  if (iso === null || iso === undefined || iso === "") {
    return null;
  }
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? null : date;
}

/**
 * A compact age: `just now`, `4m ago`, `3h ago`, `12d ago`. The clock on the
 * orchestrator and the clock in the browser are not the same clock, so a
 * timestamp in the future is skew, not the future: it clamps to `just now`
 * rather than counting backwards.
 */
export function formatRelative(
  iso: string | null | undefined,
  now: Date = new Date(),
): string {
  const date = parse(iso);
  if (date === null) {
    return PLACEHOLDER;
  }

  const seconds = Math.round((now.getTime() - date.getTime()) / 1000);
  if (seconds < MINUTE) {
    return "just now";
  }
  if (seconds < HOUR) {
    return `${Math.floor(seconds / MINUTE)}m ago`;
  }
  if (seconds < DAY) {
    return `${Math.floor(seconds / HOUR)}h ago`;
  }
  return `${Math.floor(seconds / DAY)}d ago`;
}

const DATE_TIME = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "short",
});

/** The absolute timestamp, in the viewer's locale and zone. */
export function formatDateTime(iso: string | null | undefined): string {
  const date = parse(iso);
  return date === null ? PLACEHOLDER : DATE_TIME.format(date);
}

/**
 * Accumulated cost, at a fixed number of decimals so a column of them lines
 * up. Two decimals read best in a cross-project roll-up; a single session's
 * spend is often a fraction of a cent, so the per-session lists ask for four.
 */
export function formatUsd(
  value: number | null | undefined,
  decimals = 2,
): string {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    return PLACEHOLDER;
  }
  return `$${value.toFixed(decimals)}`;
}

/** How many characters of a commit id are enough to recognise it. */
const SHORT_SHA = 7;

/** The first seven characters of a commit id, as git itself abbreviates one. */
export function shortSha(commit: string | null | undefined): string {
  if (commit === null || commit === undefined || commit === "") {
    return PLACEHOLDER;
  }
  return commit.slice(0, SHORT_SHA);
}
