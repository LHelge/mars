// Every revision and decision on one task, oldest first as `TaskDetail`
// delivers them (`SPEC.md`, "Tasks").
//
// A timeline rather than a table: hand-offs are a sequence of revisions and
// decisions on one piece of work, and reading down them is reading the
// argument. The current one is marked, because an approval two rows up is
// about code that a later revision replaced.

import { Link } from "react-router";

import type { Handoff, TaskComment, TaskDetail } from "../types";
import {
  formatDateTime,
  formatRelative,
  shortId,
  shortSha,
} from "../utils/format";
import { commentExcerpt, handoffComment, reviewLabel } from "./handoffRules";
import { CHIP } from "./taskChrome";
import { useUsername } from "./useUsername";
import { ViewDiffButton } from "./ViewDiffButton";

export interface HandoffHistoryProps {
  task: TaskDetail;
  onViewDiff: (handoff: Handoff) => void;
}

export function HandoffHistory({ task, onViewDiff }: HandoffHistoryProps) {
  if (task.handoffs.length === 0) {
    return null;
  }

  return (
    <div className="space-y-2">
      <p className="text-console-muted text-xs">History, oldest first</p>
      <ol className="border-console-border space-y-2 border-l pl-3">
        {task.handoffs.map((handoff) => (
          <HistoryRow
            key={handoff.id}
            handoff={handoff}
            comment={handoffComment(task.comments, handoff)}
            current={handoff.id === task.handoff?.id}
            onViewDiff={onViewDiff}
          />
        ))}
      </ol>
    </div>
  );
}

function HistoryRow({
  handoff,
  comment,
  current,
  onViewDiff,
}: {
  handoff: Handoff;
  comment: TaskComment | null;
  current: boolean;
  onViewDiff: (handoff: Handoff) => void;
}) {
  const label = reviewLabel(handoff);
  const username = useUsername(handoff.created_by_user_id);

  return (
    <li className={current ? "text-console-text" : "text-console-muted"}>
      <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
        <time
          dateTime={handoff.created_at}
          title={formatDateTime(handoff.created_at)}
          className="font-mono text-[0.6875rem]"
        >
          {formatRelative(handoff.created_at)}
        </time>
        {handoff.created_by_session_id !== null ? (
          <Link
            to={`/sessions/${handoff.created_by_session_id}`}
            title={handoff.created_by_session_id}
            className="text-console-accent font-mono text-[0.6875rem] hover:underline"
          >
            {shortId(handoff.created_by_session_id, 8)}
          </Link>
        ) : (
          <span className="font-mono text-[0.6875rem]">
            @{username ?? "unknown"}
          </span>
        )}
        <span className="font-mono text-[0.6875rem]" title={handoff.commit}>
          {shortSha(handoff.commit)}
        </span>
        <span className={`font-mono text-[0.6875rem] ${label.tone}`}>
          {label.text}
        </span>
        {current && (
          <span
            className={`${CHIP} border-console-accent/60 text-console-accent`}
          >
            current
          </span>
        )}
        <span className="ml-auto">
          <ViewDiffButton handoff={handoff} onViewDiff={onViewDiff} />
        </span>
      </div>
      {comment !== null && comment.body.trim() !== "" && (
        <p className="max-w-prose text-xs">{commentExcerpt(comment.body)}</p>
      )}
    </li>
  );
}
