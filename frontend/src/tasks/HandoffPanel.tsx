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
//
// `Drop hand-off` sits at the end of the same row, and only while there is a
// current hand-off to drop: it is the way back to the default branch when the
// pinned commit is known to be bad, and it asks first, with the comment the
// thread will carry (`DropHandoffConfirm.tsx`).
//
// The section opens with one line saying what a hand-off is and a link to the
// help page's `task-flow` topic, because the rest of the panel assumes it.

import { useState } from "react";
import { Link } from "react-router";

import { HelpLink } from "../components/HelpLink";
import { MarkdownBody } from "../components/Markdown";
import { SubmitButton } from "../components/SubmitButton";
import { Icon, ICON_CLASS } from "../components/icons";
import { useCopyToClipboard } from "../components/useClipboardCopy";
import { CURRENT_HANDOFF } from "../utils/testIds";
import type { Handoff, TaskDetail } from "../types";
import {
  formatDateTime,
  formatRelative,
  shortId,
  shortSha,
} from "../utils/format";
import { DropHandoffConfirm } from "./DropHandoffConfirm";
import { HandoffDiff } from "./HandoffDiff";
import { HandoffHistory } from "./HandoffHistory";
import { MergeTaskAction } from "./MergeTaskAction";
import { REVIEW_ACTION, handoffComment, reviewLabel } from "./handoffRules";
import type { ReviewDecision } from "./handoffRules";
import { ReviewForm } from "./ReviewForm";
import { RevisionForm } from "./RevisionForm";
import { useUsername } from "./useUsername";
import { ViewDiffButton } from "./ViewDiffButton";

/**
 * What a hand-off is, in the one line the section opens with (`SPEC.md`,
 * "Code hand-offs and review").
 */
const HANDOFF_HELP =
  "A hand-off pins one commit of a session's branch for the next agent to start from, with a comment and a review decision. Moving the task without one keeps the current hand-off. An approved one is merged by Mars once the task reaches an auto-merge state, and with Merge approved hand-off anywhere else.";

export interface HandoffPanelProps {
  projectId: string;
  task: TaskDetail;
}

/** Which form is open, if any. */
type OpenForm =
  | { kind: "revision" }
  | { kind: "review"; decision: ReviewDecision }
  | { kind: "drop" };

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
      <p className="text-console-muted max-w-prose text-xs">
        {HANDOFF_HELP} <HelpLink topic="task-flow" />
      </p>

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

        {current !== null && (
          <SubmitButton
            type="button"
            variant="ghost"
            disabled={open?.kind === "drop"}
            onClick={() => {
              setOpen({ kind: "drop" });
            }}
          >
            Drop hand-off
          </SubmitButton>
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

      {open?.kind === "drop" && current !== null && (
        <DropHandoffConfirm projectId={projectId} task={task} onDone={close} />
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

      <HandoffHistory task={task} onViewDiff={viewDiff} />
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
    <div
      data-testid={CURRENT_HANDOFF}
      className="border-console-border bg-console-bg space-y-2 rounded border p-3"
    >
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
 * The commit, abbreviated with the full id in its `title` and on the
 * clipboard. A commit id is something people paste into a terminal, so the
 * copy is the point; the clipboard is a permission, so a refusal falls back to
 * a selectable field rather than failing silently.
 */
function CopyCommit({ commit }: { commit: string }) {
  const { copied, manual, copy, fieldRef } = useCopyToClipboard(commit);

  return (
    <span className="inline-flex min-w-0 items-center gap-1.5">
      <span className="text-console-text font-mono text-xs" title={commit}>
        {shortSha(commit)}
      </span>
      <button
        type="button"
        aria-label="Copy commit id"
        onClick={copy}
        className="text-console-muted hover:text-console-text border-console-border hover:bg-console-raised inline-flex items-center gap-1.5 rounded border px-1.5 py-px font-mono text-[0.6875rem]"
      >
        {copied ? (
          <Icon.copied aria-hidden="true" className={ICON_CLASS} />
        ) : (
          <Icon.copy aria-hidden="true" className={ICON_CLASS} />
        )}
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
