// `/help` (`SPEC.md`, "Frontend", Help): every topic of `HELP_TOPICS` on one
// page, a table of contents beside them, and each section anchored at its
// topic so `helpPath(topic)` — `/help#<topic>` — lands on it.
//
// React Router does not scroll to a hash, and a first visit to `/help#…` renders
// the sections only once this lazy chunk has arrived, after the browser has
// already given up looking for the anchor. So the page scrolls itself: on every
// navigation that names a topic, including a second click on the same entry,
// it brings the section into view and moves focus to its heading, so keyboard
// and screen-reader users land where sighted ones do.

import { useEffect } from "react";
import { Link, useLocation } from "react-router";

import { MarkdownBody } from "../components/Markdown";
import { PageLayout } from "../components/PageLayout";
import { helpContent } from "../help/content";
import {
  HELP_TOPIC_TITLES,
  HELP_TOPICS,
  helpPath,
  parseHelpTopic,
} from "../help/topics";
import type { HelpTopic } from "../help/topics";
import { TAP_INLINE } from "../components/fieldStyles";

function headingId(topic: HelpTopic): string {
  return `${topic}-heading`;
}

export function HelpPage() {
  const { hash, key } = useLocation();
  const current = parseHelpTopic(hash.replace(/^#/, ""));

  useEffect(() => {
    if (current === undefined) {
      return;
    }
    document.getElementById(current)?.scrollIntoView({ block: "start" });
    document.getElementById(headingId(current))?.focus({ preventScroll: true });
    // `key` changes on every navigation, so following the entry already in
    // the hash scrolls back to it after the reader has scrolled away.
  }, [current, key]);

  return (
    <PageLayout title="Help">
      <div className="grid gap-6 lg:grid-cols-[13rem_minmax(0,1fr)] lg:gap-10">
        <nav
          aria-label="Help topics"
          className="lg:sticky lg:top-4 lg:self-start"
        >
          <ul className="border-console-border flex flex-wrap gap-x-4 gap-y-1 border-b pb-3 text-sm lg:flex-col lg:border-b-0 lg:border-l lg:pb-0">
            {HELP_TOPICS.map((topic) => {
              const active = topic === current;
              return (
                <li key={topic}>
                  <Link
                    to={helpPath(topic)}
                    aria-current={active ? "location" : undefined}
                    className={[
                      `-ml-px block py-0.5 lg:border-l-2 lg:pl-3 ${TAP_INLINE}`,
                      active
                        ? "border-console-accent text-console-text"
                        : "text-console-muted hover:text-console-text border-transparent",
                    ].join(" ")}
                  >
                    {HELP_TOPIC_TITLES[topic]}
                  </Link>
                </li>
              );
            })}
          </ul>
        </nav>

        <div className="flex max-w-3xl min-w-0 flex-col gap-8">
          {HELP_TOPICS.map((topic) => (
            <section
              key={topic}
              id={topic}
              aria-labelledby={headingId(topic)}
              className="scroll-mt-4"
            >
              <div className="border-console-border mb-2 flex flex-wrap items-baseline gap-x-3 border-b pb-1">
                <h2
                  id={headingId(topic)}
                  tabIndex={-1}
                  className="text-console-text text-sm font-semibold tracking-tight"
                >
                  {HELP_TOPIC_TITLES[topic]}
                </h2>
                {/* The anchor itself, so a reader can copy where this is. */}
                <Link
                  to={helpPath(topic)}
                  className="text-console-muted hover:text-console-text font-mono text-xs"
                >
                  #{topic}
                </Link>
              </div>
              <MarkdownBody appLinks className="text-console-text text-sm">
                {helpContent(topic)}
              </MarkdownBody>
            </section>
          ))}
        </div>
      </div>
    </PageLayout>
  );
}
