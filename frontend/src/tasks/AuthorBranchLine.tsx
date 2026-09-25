// The quiet state line of a task waiting for its author session's branch to
// reach the default branch (`SPEC.md`, "Frontend", Task board;
// `tasks/authorBranch.ts` for the rule).
//
// It is a link to the project's Branches tab, where that branch is merged. The
// tone is the parked colour: nothing is wrong, the task is waiting on someone,
// and the card should read as paused rather than alarmed. The card and the
// drawer both render it, the card as a footer outside its own link, since a
// link cannot hold another.

import { Link } from "react-router";

import { Icon, ICON_CLASS } from "../components/icons";
import type { Task } from "../types";
import { AUTHOR_BRANCH_WAIT } from "../utils/testIds";
import { branchesPath } from "./taskLink";
import { useAuthorBranchWait } from "./useAuthorBranchWait";

export interface AuthorBranchLineProps {
  task: Task;
  /** Classes for the wrapper, which exists only while there is a line. */
  className?: string;
}

export function AuthorBranchLine({ task, className }: AuthorBranchLineProps) {
  const message = useAuthorBranchWait(task);

  if (message === null) {
    return null;
  }

  return (
    <div className={className}>
      <Link
        to={branchesPath(task.project_id)}
        data-testid={AUTHOR_BRANCH_WAIT}
        title="Merge the branch from the project's Branches tab; the dispatcher launches the task after that"
        className="text-state-parked hover:text-console-text flex items-start gap-1.5 text-xs"
      >
        <Icon.branch aria-hidden="true" className={`mt-px ${ICON_CLASS}`} />
        <span className="min-w-0">{message}</span>
      </Link>
    </div>
  );
}
