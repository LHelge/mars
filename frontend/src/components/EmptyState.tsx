// What a list says when it has nothing in it: an invitation to act, never an
// apology.

import type { ReactNode } from "react";

export interface EmptyStateProps {
  title: string;
  description?: string;
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
      {description && (
        <p className="text-console-muted max-w-prose text-sm">{description}</p>
      )}
      {action && <div className="pt-1">{action}</div>}
    </div>
  );
}
