// The prose of each help topic: one Markdown file per topic beside this module,
// imported as text and rendered by `MarkdownBody` (`SPEC.md`, "Frontend",
// Help). Only `HelpPage` imports this, so the prose is in the `/help` chunk
// and never in what a first paint fetches.
//
// A link from one topic to another is written `[words](help:<topic>)` and
// resolved here through `helpPath`, so no Markdown file spells a route by hand
// and a topic that does not exist is caught by `content.test.ts` rather than
// shipped as a dead link.

import agentCredentials from "./agent-credentials.md?raw";
import automation from "./automation.md?raw";
import branches from "./branches.md?raw";
import gettingStarted from "./getting-started.md?raw";
import gitCredential from "./git-credential.md?raw";
import instructions from "./instructions.md?raw";
import profiles from "./profiles.md?raw";
import secrets from "./secrets.md?raw";
import sharedDirectories from "./shared-directories.md?raw";
import skills from "./skills.md?raw";
import taskFlow from "./task-flow.md?raw";
import { helpPath, parseHelpTopic } from "./topics";
import type { HelpTopic } from "./topics";

/** Each topic's Markdown as written, `help:` links unresolved. */
export const HELP_SOURCES: Record<HelpTopic, string> = {
  "getting-started": gettingStarted,
  "git-credential": gitCredential,
  "agent-credentials": agentCredentials,
  secrets,
  profiles,
  instructions,
  skills,
  automation,
  branches,
  "task-flow": taskFlow,
  "shared-directories": sharedDirectories,
};

/** A Markdown link destination of the form `(help:<topic>)`. */
const HELP_LINK = /\]\(help:([a-z-]+)\)/g;

/** Every `help:` target a text names, known topic or not. */
export function helpLinkTargets(markdown: string): string[] {
  return [...markdown.matchAll(HELP_LINK)].map((match) => match[1] ?? "");
}

/**
 * The text with every `help:<topic>` destination replaced by that topic's
 * `helpPath`. An unknown topic is left as written; `react-markdown` drops the
 * unrecognised scheme, and the content test fails on it first.
 */
export function resolveHelpLinks(markdown: string): string {
  return markdown.replace(HELP_LINK, (whole, name: string) => {
    const topic = parseHelpTopic(name);
    return topic === undefined ? whole : `](${helpPath(topic)})`;
  });
}

/** One topic's Markdown, ready for `MarkdownBody`. */
export function helpContent(topic: HelpTopic): string {
  return resolveHelpLinks(HELP_SOURCES[topic]);
}
