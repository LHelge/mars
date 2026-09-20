// A task's comments (`SPEC.md`, "Frontend", "Task board": "comments (system
// comments styled apart)").
//
// A timeline, oldest first, hung off one rule down the left: the order is the
// argument, so the eye should be able to run down it without stopping at a
// card border on every entry. A system comment — the tracker narrating a
// claim, a release, an escalation — is the same shape set quieter and in
// italic, because it is context for the conversation rather than part of it.

import { Link } from "react-router";

import { MarkdownBody } from "../components/Markdown";
import type { Comment } from "../types";
import { formatDateTime, formatRelative, shortId } from "../utils/format";
import { CHIP } from "./taskChrome";
import { useUsername } from "./useUsername";

export interface CommentListProps {
  comments: Comment[];
}

export function CommentList({ comments }: CommentListProps) {
  if (comments.length === 0) {
    return (
      <p className="text-console-muted text-sm">
        No comments yet. Anything written here is visible to the agents working
        this task.
      </p>
    );
  }

  // Oldest first, whatever order the payload arrived in.
  const ordered = [...comments].sort((left, right) =>
    left.created_at.localeCompare(right.created_at),
  );

  return (
    <ol className="border-console-border space-y-3 border-l pl-3">
      {ordered.map((comment) => (
        <CommentEntry key={comment.id} comment={comment} />
      ))}
    </ol>
  );
}

function CommentEntry({ comment }: { comment: Comment }) {
  return (
    <li className={comment.system ? "text-console-muted italic" : undefined}>
      <div className="flex flex-wrap items-baseline gap-2">
        <Author comment={comment} />
        <time
          dateTime={comment.created_at}
          title={formatDateTime(comment.created_at)}
          className="text-console-muted font-mono text-[0.6875rem]"
        >
          {formatRelative(comment.created_at)}
        </time>
      </div>
      <div
        className={`mt-0.5 max-w-prose text-sm ${comment.system ? "" : "text-console-text"}`}
      >
        <MarkdownBody>{comment.body}</MarkdownBody>
      </div>
    </li>
  );
}

/**
 * Who wrote it: a username, the session that wrote it (a link into its
 * transcript) or the tracker itself. A system comment has no author at all
 * (`docs/data-model.md`, `task_comments`), so it is labelled rather than
 * attributed.
 */
function Author({ comment }: { comment: Comment }) {
  const username = useUsername(comment.system ? null : comment.author_user_id);

  if (comment.system) {
    return (
      <span
        className={`${CHIP} text-console-muted not-italic`}
        title="Written by the tracker itself"
      >
        system
      </span>
    );
  }

  if (comment.author_session_id !== null) {
    return (
      <Link
        to={`/sessions/${comment.author_session_id}`}
        title={`Session ${comment.author_session_id}`}
        className="text-console-accent font-mono text-xs hover:underline"
      >
        {shortId(comment.author_session_id, 8)}
      </Link>
    );
  }

  if (username !== null) {
    return (
      <span className="text-console-text font-mono text-xs">@{username}</span>
    );
  }

  return <span className="text-console-muted font-mono text-xs">unknown</span>;
}
