// What a list says when it has nothing in it: an invitation to act, never an
// apology.

import type { ReactNode } from "react";

import type { HelpTopic } from "../help/topics";
import { HelpLink } from "./HelpLink";

export interface EmptyStateProps {
  title: string;
  description?: string;
  /** The help topic a `Learn more` link after the description opens. */
  help?: HelpTopic;
  /** Usually the same button the section header offers. */
  action?: ReactNode;
  /**
   * `warning` for the few empty lists that are a missing prerequisite rather
   * than an empty start — something later will fail until it is filled in.
   */
  tone?: "default" | "warning";
}

export function EmptyState({
  title,
  description,
  help,
  action,
  tone = "default",
}: EmptyStateProps) {
  const warn = tone === "warning";
  return (
    <div
      className={`flex flex-col items-start gap-2 rounded border border-dashed px-4 py-8 ${
        warn ? "border-state-parked/60" : "border-console-border"
      }`}
    >
      <p
        className={`text-sm ${warn ? "text-state-parked" : "text-console-text"}`}
      >
        {title}
      </p>
      {(description || help) && (
        <div className="text-console-muted max-w-prose text-sm">
          {description && <p className="inline">{description}</p>}
          {description && help && " "}
          {help && <HelpLink topic={help} />}
        </div>
      )}
      {action && <div className="pt-1">{action}</div>}
    </div>
  );
}
