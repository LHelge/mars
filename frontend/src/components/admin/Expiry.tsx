// How long an invitation has left (`SPEC.md`, "User-facing features", Login
// and invites: a link is good for 7 days). Its own module because both the
// panel and its rows read it.

import {
  formatDateTime,
  formatRelative,
  PLACEHOLDER,
} from "../../utils/format";

const MINUTE = 60;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/**
 * How long is left, as `in 6d` / `in 3h` / `in 12m`. `formatRelative` measures
 * age and clamps the future away, so a deadline needs its own reading; a
 * deadline already past is handed back to `formatRelative` and shown in the
 * failed-state colour.
 */
function untilLabel(iso: string, now: Date): string | null {
  const at = new Date(iso).getTime();
  if (Number.isNaN(at)) {
    return null;
  }
  const seconds = Math.round((at - now.getTime()) / 1000);
  if (seconds <= 0) {
    return null;
  }
  if (seconds < MINUTE) {
    return "in under a minute";
  }
  if (seconds < HOUR) {
    return `in ${Math.floor(seconds / MINUTE)}m`;
  }
  if (seconds < DAY) {
    return `in ${Math.floor(seconds / HOUR)}h`;
  }
  return `in ${Math.floor(seconds / DAY)}d`;
}

/**
 * An invite the reaper has not collected yet is still listed; it reads as
 * expired and keeps its Resend action, because a resend issues a new expiry.
 */
export function Expiry({ iso, now = new Date() }: { iso: string; now?: Date }) {
  if (Number.isNaN(new Date(iso).getTime())) {
    return <span className="text-console-muted">{PLACEHOLDER}</span>;
  }
  const remaining = untilLabel(iso, now);
  return (
    <span
      title={formatDateTime(iso)}
      className={
        remaining === null
          ? "text-state-failed font-mono text-xs"
          : "text-console-muted font-mono text-xs"
      }
    >
      {remaining ?? `expired ${formatRelative(iso, now)}`}
    </span>
  );
}
