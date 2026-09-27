// The board's key: one line naming every glyph and chip the columns and the
// cards use (`SPEC.md`, "Frontend", "Task board"). The marks are terse by
// design — `▸`, `↑2`, `P1` — and a tooltip is no place for what they mean,
// because a phone has no hover (`SPEC.md`, "Frontend", "Mobile layout"). Each
// chip also carries its meaning as its accessible name (`chipMeaning`), so
// this line is for the eye.
//
// It is a disclosure, shut by default: the operator who knows the marks keeps
// the board's height, and the one who does not is one tap from the answer.

import { TAP_INLINE } from "../components/fieldStyles";
import type { TaskStateKind } from "../types";
import { CHIP } from "./taskChrome";
import { KIND_COLOUR, KIND_MARK } from "./taskStateRules";

const KINDS: readonly TaskStateKind[] = ["queue", "human", "terminal"];

/** Each card chip as it looks, and what it means in a word or three. */
const CHIPS: readonly { mark: string; meaning: string; tone?: string }[] = [
  { mark: "P0–P3", meaning: "priority" },
  { mark: "part of #n", meaning: "child task" },
  {
    mark: "blocked",
    meaning: "waiting on children or dependencies",
    tone: "border-state-failed/60 text-state-failed",
  },
  { mark: "↑n ↓n", meaning: "blocked by / blocks n tasks" },
  { mark: "n attempts", meaning: "sessions that picked it up" },
  { mark: "round n/m", meaning: "revision rounds of the limit" },
  { mark: "@user", meaning: "assignee" },
  { mark: "held", meaning: "the session holding it" },
  { mark: "auto-merge", meaning: "column merges approved hand-offs" },
];

export function BoardLegend() {
  return (
    <details className="text-console-muted text-xs">
      <summary
        className={`hover:text-console-text w-fit cursor-pointer select-none ${TAP_INLINE}`}
      >
        Key to the marks
      </summary>
      <p className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1">
        {KINDS.map((kind) => (
          <span key={kind} className="whitespace-nowrap">
            <span
              aria-hidden="true"
              className={`font-mono ${KIND_COLOUR[kind]}`}
            >
              {KIND_MARK[kind]}
            </span>{" "}
            {kind}
          </span>
        ))}
        {CHIPS.map((chip) => (
          <span key={chip.mark} className="whitespace-nowrap">
            <span className={`${CHIP} ${chip.tone ?? "text-console-muted"}`}>
              {chip.mark}
            </span>{" "}
            {chip.meaning}
          </span>
        ))}
      </p>
    </details>
  );
}
