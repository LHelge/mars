// What the three git forms share that is not markup: how an action's three
// answers are held, how a form tells the panel it is working, and how a
// preselected ref is reconciled with the refs the mirror really has.
//
// The orchestrator serialises git work per project, so a second merge started
// while the first one runs only waits. The panel therefore disables every form
// while any one of them is in flight, and a form reports its own state here
// rather than the panel reaching into it.

import { useEffect, useState } from "react";
import { useFormSubmit } from "../../hooks/useFormSubmit";
import { isGitConflict } from "../../services/git";
import type { Branch } from "../../types";
import { githubCompareUrl } from "../../utils/github";

/** The conflicting paths of a 422, with the server's own sentence. */
export interface GitConflict {
  paths: string[];
  message: string;
}

/** One attempt at a git action: what it produced, or what stopped it. */
export interface GitAction<T> {
  submit: () => void;
  loading: boolean;
  /** The one refusal that is not a conflict, in `useFormSubmit`'s words. */
  error: string | null;
  reset: () => void;
  conflict: GitConflict | null;
  /** What the last attempt produced, or `null` if it produced nothing. */
  result: T | null;
}

/**
 * A git action and its three answers.
 *
 * Every git form here handles a conflict rather than failing on it: a 422
 * carries the paths git could not merge, which is a list to read and not an
 * error banner (`SPEC.md`, "Git": conflicts come back as `{status, error,
 * conflicts}`). The outcome and the conflict are cleared together when the
 * next attempt starts, so a form never shows one attempt's paths beside
 * another's result.
 *
 * `run` returns `null` for an attempt that produced nothing to report — a
 * refusal the caller words itself, as a rejected push does.
 */
export function useGitAction<T>(run: () => Promise<T | null>): GitAction<T> {
  const [result, setResult] = useState<T | null>(null);
  const [conflict, setConflict] = useState<GitConflict | null>(null);

  const form = useFormSubmit(async () => {
    setResult(null);
    setConflict(null);
    try {
      setResult(await run());
    } catch (caught) {
      if (isGitConflict(caught)) {
        setConflict({ paths: caught.conflicts, message: caught.error });
        return;
      }
      throw caught;
    }
  });

  return {
    submit: () => {
      void form.submit();
    },
    loading: form.loading,
    error: form.error,
    reset: form.reset,
    conflict,
    result,
  };
}

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
export function defaultRemoteBranch(ref: string, isSession: boolean): string {
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
