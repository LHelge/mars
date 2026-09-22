// The shape every git form on this page has: a column of fields, then what
// the last attempt answered — the conflicting paths git reported, and the one
// refusal that is not a conflict.
//
// The tail is the shell's because the order matters and was copied three
// times: a conflict is a list to read, so it sits above the alert, and the
// alert is dismissible because a refusal describes an attempt the operator has
// since moved on from.

import type { ReactNode } from "react";

import { Alert } from "../Alert";
import { ConflictList } from "./ConflictList";
import type { GitAction } from "./formState";

export interface GitFormShellProps {
  /** The action the form submits; the shell reads its answers. */
  action: GitAction<unknown>;
  /** The fields, the button row and any note the form words itself. */
  children: ReactNode;
}

export function GitFormShell({ action, children }: GitFormShellProps) {
  return (
    <form
      className="flex flex-col gap-3"
      onSubmit={(event) => {
        event.preventDefault();
        action.submit();
      }}
    >
      {children}

      {action.conflict !== null && (
        <ConflictList
          paths={action.conflict.paths}
          message={action.conflict.message}
        />
      )}
      {action.error !== null && (
        <Alert kind="error" onDismiss={action.reset}>
          {action.error}
        </Alert>
      )}
    </form>
  );
}

/** What an action that landed reports, beside the button that ran it. */
export function GitResultNote({ children }: { children: ReactNode }) {
  return (
    <span className="text-state-running font-mono text-xs">{children}</span>
  );
}
