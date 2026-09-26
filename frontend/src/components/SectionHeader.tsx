// Heading row for a block inside a page: title on the left, its actions on the
// right, and a rule that separates the block from the one above it.

import type { ReactNode } from "react";

import type { HelpTopic } from "../help/topics";
import { HelpLink } from "./HelpLink";

export interface SectionHeaderProps {
  title: string;
  description?: string;
  /** The help topic a `Learn more` link after the description opens. */
  help?: HelpTopic;
  actions?: ReactNode;
}

export function SectionHeader({
  title,
  description,
  help,
  actions,
}: SectionHeaderProps) {
  return (
    <div className="border-console-border flex flex-wrap items-baseline justify-between gap-x-4 gap-y-2 border-b pb-2">
      <div className="min-w-0">
        <h2 className="text-console-text text-sm font-semibold tracking-tight">
          {title}
        </h2>
        {(description || help) && (
          <div className="text-console-muted max-w-prose text-xs">
            {description && <p className="inline">{description}</p>}
            {description && help && " "}
            {help && <HelpLink topic={help} />}
          </div>
        )}
      </div>
      {actions && (
        <div className="flex shrink-0 items-center gap-2">{actions}</div>
      )}
    </div>
  );
}
