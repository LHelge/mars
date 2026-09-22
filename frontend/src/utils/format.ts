// Display formatting shared by every list and detail view. Every helper here
// takes a value that may be missing and answers `PLACEHOLDER` for it, so a
// caller never has to branch before calling them.

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

const UTC_DATE_TIME = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "short",
  timeZone: "UTC",
});

/**
 * The same instant in UTC, named as such. A schedule is written in UTC and
 * read in the viewer's own zone, so the two are shown together and the UTC
 * one has to say which one it is.
 */
export function formatUtc(iso: string | null | undefined): string {
  const date = parse(iso);
  return date === null ? PLACEHOLDER : `${UTC_DATE_TIME.format(date)} UTC`;
}

/** What a single session's spend is shown to: four decimals, in one place. */
export const COST_DECIMALS = 4;

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

const TOKENS = new Intl.NumberFormat();

/**
 * A token counter. Grouped, because the interesting comparison between two
 * runs is the order of magnitude and a bare seven-digit number hides it.
 */
export function formatTokens(value: number | null | undefined): string {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    return PLACEHOLDER;
  }
  return TOKENS.format(value);
}

/** How many characters of a container or session id the header shows. */
const SHORT_ID = 12;

/**
 * The leading characters of a long opaque id — a container id, a CLI session
 * id — as the engines themselves abbreviate one. The caller keeps the full
 * value in a `title`, so nothing is lost by shortening it.
 */
export function shortId(
  value: string | null | undefined,
  length = SHORT_ID,
): string {
  if (value === null || value === undefined || value === "") {
    return PLACEHOLDER;
  }
  return value.slice(0, length);
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
