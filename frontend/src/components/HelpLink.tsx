// A quiet "Learn more" link to one section of the help page (`CLAUDE.md`,
// "Frontend conventions": shared UI). `FieldShell`, `SectionHeader`,
// `EmptyState` and the profile editor's `Fieldset` render it beside their
// hint or description when given `help`, never inside the text a control's
// `aria-describedby` names, so a screen reader reads the hint and not a link.
//
// It takes the surrounding text size, so it sits as one more phrase of the
// line it follows. The visible words are the same everywhere; the accessible
// name adds the topic, so a page with several of them has links that say
// where each goes.

import type { ReactNode } from "react";
import { Link } from "react-router";

import { HELP_TOPIC_TITLES, helpPath } from "../help/topics";
import type { HelpTopic } from "../help/topics";
import { TAP_INLINE } from "./fieldStyles";

export interface HelpLinkProps {
  topic: HelpTopic;
  /** The visible words; "Learn more" when omitted. */
  children?: ReactNode;
}

export function HelpLink({ topic, children }: HelpLinkProps) {
  return (
    <Link
      to={helpPath(topic)}
      className={`text-console-accent underline-offset-2 hover:underline focus-visible:underline ${TAP_INLINE}`}
    >
      {children ?? (
        <>
          Learn more{" "}
          <span className="sr-only">about {HELP_TOPIC_TITLES[topic]}</span>
        </>
      )}
    </Link>
  );
}
