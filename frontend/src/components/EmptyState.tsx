// What a list says when it has nothing in it: an invitation to act, never an
// apology.

import type { ReactNode } from "react";

export interface EmptyStateProps {
  title: string;
  description?: string;
  /** Usually the same button the section header offers. */
  action?: ReactNode;
}

export function EmptyState({ title, description, action }: EmptyStateProps) {
  return (
    <div className="border-console-border flex flex-col items-start gap-2 rounded border border-dashed px-4 py-8">
      <p className="text-console-text text-sm">{title}</p>
      {description && (
        <p className="text-console-muted max-w-prose text-sm">{description}</p>
      )}
      {action && <div className="pt-1">{action}</div>}
    </div>
  );
}
