// What the three git forms share: the control styling of the console's inputs
// and the way a form tells the panel it is working.
//
// The orchestrator serialises git work per project, so a second merge started
// while the first one runs only waits. The panel therefore disables every form
// while any one of them is in flight, and a form reports its own state here
// rather than the panel reaching into it.

import { useEffect } from "react";
import type { Branch } from "../../types";
import { githubCompareUrl } from "../../utils/github";

/** `(form id, in flight)`, as `GitActionsPanel` tracks it. */
export type ReportBusy = (id: string, busy: boolean) => void;

/** Keeps the panel's "something is running" flag in step with one form. */
export function useReportBusy(
  id: string,
  busy: boolean,
  report: ReportBusy,
): void {
  useEffect(() => {
    report(id, busy);
    return () => {
      report(id, false);
    };
  }, [id, busy, report]);
}

/** The refs of one kind, in the order the server listed them. */
export function refsOfKind(branches: Branch[], kind: Branch["kind"]): Branch[] {
  return branches.filter((branch) => branch.kind === kind);
}

/**
 * The name a push sends a ref to by default (`ARCHITECTURE.md`, "Git model":
 * session refs are pushed as `refs/heads/session/<id>`); an integration head
 * keeps its own name.
 */
export function defaultRemoteBranch(
  ref: string,
  isSession: boolean,
): string {
  return isSession ? `session/${ref}` : ref;
}

/**
 * The branch lists arrive after a form first renders, and a project's
 * `default_branch` is only a name: a preselection the mirror turns out not to
 * hold would otherwise be submitted and refused.
 *
 * `chosenOr` keeps the choice while the options do not contradict it and falls
 * back to the first real option once they do. `keptIfKnown` empties it
 * instead, for a control with no sensible first answer.
 */
export function chosenOr(value: string, options: Branch[]): string {
  if (options.length === 0 || options.some((one) => one.name === value)) {
    return value;
  }
  return options[0]?.name ?? "";
}

export function keptIfKnown(value: string, options: Branch[]): string {
  if (options.length === 0 || options.some((one) => one.name === value)) {
    return value;
  }
  return "";
}

/**
 * The compare link a push earned, or `null`. A head pushed under its own name
 * compares against itself, which is an empty page on GitHub, so it gets none.
 */
export function compareUrlFor(
  remoteUrl: string,
  target: string | null,
  remoteBranch: string,
): string | null {
  if (target === null || target === remoteBranch) {
    return null;
  }
  return githubCompareUrl(remoteUrl, target, remoteBranch);
}
