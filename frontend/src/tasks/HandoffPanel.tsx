// The drawer's hand-off section (`SPEC.md`, "Frontend", "Hand-off controls";
// `ARCHITECTURE.md`, "Task tracker", "Code hand-offs" and "Review approval").
//
// A task's hand-off answers one question — which commit is the next agent
// supposed to start from, and has anyone looked at it — so the panel puts that
// answer first, in one block: source, branch, commit, what the author said
// about it, and the review badge. The badge is the only coloured thing in the
// drawer's body, because "approved" and "changes requested" are the two facts
// a person scans for. Everything else is the console's ordinary grey.
//
// Underneath sits the history, oldest first as the API delivers it, as a
// timeline rather than a table: hand-offs are a sequence of revisions and
// decisions on one piece of work, and reading down them is reading the
// argument. The current one is marked, because an approval two rows up is
// about code that a later revision replaced.
//
// The controls live here rather than in the action bar above. Publishing a
// revision and reviewing one are both statements about the hand-off the panel
// is showing, and both open a form wider than a button row: keeping them in
// the same section means the commit being talked about is never off-screen
// from the form talking about it.
//
// The merge control and the diff viewer fill the seams the panel was built
// with: `Merge approved hand-off` sits beside the review badge, because it is
// that badge being acted on, and `View diff` on any row opens the retained
// commit's diff inside the section rather than navigating away from the
// argument it belongs to.

import { useEffect, useRef, useState } from "react";
import { Link } from "react-router";

import { MarkdownBody } from "../components/Markdown";
import { SubmitButton } from "../components/SubmitButton";
import type { Comment, Handoff, TaskDetail } from "../types";
import { formatDateTime, formatRelative, shortId } from "../utils/format";
import { HandoffDiff } from "./HandoffDiff";
import { MergeTaskAction } from "./MergeTaskAction";
import {
  REVIEW_ACTION,
  commentExcerpt,
  handoffComment,
  reviewLabel,
} from "./handoffRules";
import type { ReviewDecision } from "./handoffRules";
import { shortCommit } from "./launchRules";
import { ReviewForm } from "./ReviewForm";
import { RevisionForm } from "./RevisionForm";
import { CHIP } from "./taskChrome";
import { useUsername } from "./useUsername";

export interface HandoffPanelProps {
  projectId: string;
  task: TaskDetail;
}

/** Which form is open, if any. */
type OpenForm =
  { kind: "revision" } | { kind: "review"; decision: ReviewDecision };

export function HandoffPanel({ projectId, task }: HandoffPanelProps) {
  const [open, setOpen] = useState<OpenForm | null>(null);
  // Which hand-off's retained commit is on screen. The panel holds the id
  // rather than the row, so a refreshed task carries the view with it.
  const [viewing, setViewing] = useState<string | null>(null);
  const current = task.handoff;

  function close() {
    setOpen(null);
  }

  const shown =
    viewing === null
      ? null
      : (task.handoffs.find((one) => one.id === viewing) ?? null);

  function viewDiff(handoff: Handoff) {
    // Clicking the row that is already open closes it, so the button is the
    // same affordance both ways.
    setViewing((shownId) => (shownId === handoff.id ? null : handoff.id));
  }

  return (
    <div className="space-y-4">
      {current === null ? (
        <p className="text-console-muted text-sm">No code hand-off</p>
      ) : (
        <CurrentHandoff
          projectId={projectId}
          task={task}
          handoff={current}
          onViewDiff={viewDiff}
        />
      )}

      <div className="flex flex-wrap items-center gap-2">
        <SubmitButton
          type="button"
          variant="ghost"
          disabled={open?.kind === "revision"}
          onClick={() => {
            setOpen({ kind: "revision" });
          }}
        >
          Publish revision
        </SubmitButton>

        {(["approved", "changes_requested", "none"] as const).map(
          (decision) => (
            <span
              key={decision}
              title={
                current === null
                  ? "There is nothing to review until a revision is published"
                  : undefined
              }
            >
              <SubmitButton
                type="button"
                variant="ghost"
                disabled={
                  current === null ||
                  (open?.kind === "review" && open.decision === decision)
                }
                onClick={() => {
                  setOpen({ kind: "review", decision });
                }}
              >
                {REVIEW_ACTION[decision]}
              </SubmitButton>
            </span>
          ),
        )}
      </div>

      {open?.kind === "revision" && (
        <RevisionForm projectId={projectId} task={task} onDone={close} />
      )}

      {open?.kind === "review" && current !== null && (
        <ReviewForm
          // A fresh form per decision: the comment written to approve is not
          // the comment written to reject. Not per hand-off: the form pins the
          // one it opened on and says when a newer revision arrives, rather
          // than throwing away a half-written review.
          key={open.decision}
          projectId={projectId}
          task={task}
          handoff={current}
          decision={open.decision}
          onDone={close}
        />
      )}

      {shown !== null && (
        <HandoffDiff
          // A fresh query per hand-off, rather than one that swaps its subject.
          key={shown.id}
          projectId={projectId}
          handoff={shown}
          onClose={() => {
            setViewing(null);
          }}
        />
      )}

      <History task={task} onViewDiff={viewDiff} />
    </div>
  );
}

/**
 * The hand-off the next agent would start from: where the code is, what was
 * said about it, and whether anyone has judged it.
 */
function CurrentHandoff({
  projectId,
  task,
  handoff,
  onViewDiff,
}: {
  projectId: string;
  task: TaskDetail;
  handoff: Handoff;
  onViewDiff: (handoff: Handoff) => void;
}) {
  const label = reviewLabel(handoff);
  const comment = handoffComment(task.comments, handoff);

  return (
    <div className="border-console-border bg-console-bg space-y-2 rounded border p-3">
      <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
        {handoff.source_session_id === null ? (
          <span className="text-console-muted font-mono text-xs">
            deleted session
          </span>
        ) : (
          <Link
            to={`/sessions/${handoff.source_session_id}`}
            title={handoff.source_session_id}
            className="text-console-accent font-mono text-xs hover:underline"
          >
            {shortId(handoff.source_session_id, 8)}
          </Link>
        )}
        <span
          className="text-console-text font-mono text-xs"
          title={handoff.source_branch}
        >
          {handoff.source_branch}
        </span>
        <CopyCommit commit={handoff.commit} />
        <ViewDiffButton handoff={handoff} onViewDiff={onViewDiff} />
        {/* The task merge control sits beside the badge: it is the action
            that an approval of this commit unlocks. */}
        <span className={`ml-auto font-mono text-xs ${label.tone}`}>
          {label.text}
        </span>
        <MergeTaskAction projectId={projectId} task={task} />
      </div>

      <Reviewer handoff={handoff} />

      {comment === null ? (
        <p className="text-console-muted text-sm">comment unavailable</p>
      ) : (
        <div className="text-console-text text-sm">
          <MarkdownBody>{comment.body}</MarkdownBody>
        </div>
      )}
    </div>
  );
}

/** Who made the review decision and when, for a hand-off that has one. */
function Reviewer({ handoff }: { handoff: Handoff }) {
  const username = useUsername(handoff.reviewed_by_user_id);

  if (handoff.review_status === "unreviewed" || handoff.reviewed_at === null) {
    return null;
  }

  return (
    <p className="text-console-muted text-xs">
      by{" "}
      {handoff.reviewed_by_session_id !== null ? (
        <Link
          to={`/sessions/${handoff.reviewed_by_session_id}`}
          title={handoff.reviewed_by_session_id}
          className="text-console-accent font-mono hover:underline"
        >
          {shortId(handoff.reviewed_by_session_id, 8)}
        </Link>
      ) : (
        <span className="font-mono">@{username ?? "someone"}</span>
      )}{" "}
      <time
        dateTime={handoff.reviewed_at}
        title={formatDateTime(handoff.reviewed_at)}
      >
        {formatRelative(handoff.reviewed_at)}
      </time>
    </p>
  );
}

/**
 * Every revision and decision on this task, oldest first as `TaskDetail`
 * delivers them (`SPEC.md`, "Tasks").
 */
function History({
  task,
  onViewDiff,
}: {
  task: TaskDetail;
  onViewDiff: (handoff: Handoff) => void;
}) {
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
  comment: Comment | null;
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
          {shortCommit(handoff.commit)}
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

/** Opens the retained commit of one hand-off, wherever it is listed. */
function ViewDiffButton({
  handoff,
  onViewDiff,
}: {
  handoff: Handoff;
  onViewDiff: (handoff: Handoff) => void;
}) {
  return (
    <button
      type="button"
      onClick={() => {
        onViewDiff(handoff);
      }}
      className="border-console-border hover:bg-console-raised hover:text-console-text rounded border px-1.5 py-px font-mono text-[0.6875rem]"
    >
      View diff
    </button>
  );
}

/**
 * The commit, abbreviated with the full id in its `title` and on the
 * clipboard. A commit id is something people paste into a terminal, so the
 * copy is the point; the clipboard is a permission, so a refusal falls back to
 * a selectable field rather than failing silently.
 */
function CopyCommit({ commit }: { commit: string }) {
  const [copied, setCopied] = useState(false);
  const [manual, setManual] = useState(false);
  const fieldRef = useRef<HTMLInputElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    return () => {
      if (timer.current !== null) clearTimeout(timer.current);
    };
  }, []);

  useEffect(() => {
    if (manual) fieldRef.current?.select();
  }, [manual]);

  const copy = async (): Promise<void> => {
    const clipboard = navigator.clipboard as Clipboard | undefined;
    try {
      if (clipboard === undefined) throw new Error("no clipboard");
      await clipboard.writeText(commit);
    } catch {
      setCopied(false);
      setManual(true);
      return;
    }
    setManual(false);
    setCopied(true);
    if (timer.current !== null) clearTimeout(timer.current);
    timer.current = setTimeout(() => {
      setCopied(false);
    }, 2000);
  };

  return (
    <span className="inline-flex min-w-0 items-center gap-1.5">
      <span className="text-console-text font-mono text-xs" title={commit}>
        {shortCommit(commit)}
      </span>
      <button
        type="button"
        aria-label="Copy commit id"
        onClick={() => {
          void copy();
        }}
        className="text-console-muted hover:text-console-text border-console-border hover:bg-console-raised rounded border px-1.5 py-px font-mono text-[0.6875rem]"
      >
        {copied ? "Copied" : "Copy"}
      </button>
      {manual && (
        <input
          ref={fieldRef}
          readOnly
          aria-label="Commit id"
          value={commit}
          onFocus={(event) => {
            event.target.select();
          }}
          className="bg-console-surface border-console-border text-console-muted w-72 max-w-full rounded border px-2 py-0.5 font-mono text-xs"
        />
      )}
    </span>
  );
}
