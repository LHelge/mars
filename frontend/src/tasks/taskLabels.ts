// Turning a label field into the `labels` array of `SPEC.md`, "Tasks".
//
// A label is spelled by the same rule as a task state's name — the API
// validates both against `[a-z0-9][a-z0-9_-]{0,31}` — so the pattern is
// imported rather than written out a second time; only the sentence the field
// shows is worded for labels.
//
// It lives in its own module because the form that uses it renders a
// component, and such a module exports nothing else
// (`react-refresh/only-export-components`).

import { STATE_NAME_PATTERN } from "./taskStateRules";

/** Commas and whitespace both separate; a run of either is one separator. */
const SEPARATORS = /[\s,]+/;

/** What the label field says when an entry does not match the rule. */
export const LABEL_RULE =
  "Each label is 1–32 characters of a-z, 0-9, _ or -, separated by commas or spaces";

export interface ParsedLabels {
  /** The accepted labels, in the order typed, without repeats. */
  labels: string[];
  /** The entries the API would refuse, in the order typed. */
  invalid: string[];
}

/**
 * Splits the field and sorts the entries into the two lists. Repeats are
 * dropped rather than reported: typing a label twice is a slip, not an error,
 * and the API stores a set.
 */
export function parseLabels(raw: string): ParsedLabels {
  const labels: string[] = [];
  const invalid: string[] = [];
  for (const entry of raw.split(SEPARATORS)) {
    if (entry === "") continue;
    if (!STATE_NAME_PATTERN.test(entry)) {
      if (!invalid.includes(entry)) invalid.push(entry);
    } else if (!labels.includes(entry)) {
      labels.push(entry);
    }
  }
  return { labels, invalid };
}

/** The message the field shows, or null when every entry is spelled right. */
export function labelsError(raw: string): string | null {
  const { invalid } = parseLabels(raw);
  if (invalid.length === 0) return null;
  return `${LABEL_RULE}. Not accepted: ${invalid.join(", ")}`;
}
