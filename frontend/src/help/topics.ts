// The topics of the help page (`SPEC.md`, "Frontend", Help) and where each one
// lives: a closed set, so a `HelpLink` to a topic that does not exist fails
// `tsc -b` rather than landing on the top of `/help`.
//
// Small and free of the topics' prose on purpose: `HelpLink` imports it, and
// `FieldShell`, which renders a `HelpLink`, is on the first-paint path. The
// prose is `content.ts`, which only the lazily routed `HelpPage` reaches.

/** Every topic, in the order the page's table of contents lists them. */
export const HELP_TOPICS = [
  "getting-started",
  "git-credential",
  "agent-credentials",
  "secrets",
  "profiles",
  "instructions",
  "skills",
  "automation",
  "branches",
  "task-flow",
  "shared-directories",
] as const;

export type HelpTopic = (typeof HELP_TOPICS)[number];

/** The heading each topic's section carries, and its table-of-contents entry. */
export const HELP_TOPIC_TITLES: Record<HelpTopic, string> = {
  "getting-started": "Getting started",
  "git-credential": "Git credential",
  "agent-credentials": "Agent credentials",
  secrets: "Secrets",
  profiles: "Agent profiles",
  instructions: "How a session is instructed",
  skills: "Skills",
  automation: "Automation",
  branches: "Branches and merging",
  "task-flow": "Task flow",
  "shared-directories": "Shared directories",
};

/** The topic a string names — a URL hash without its `#` — or `undefined`. */
export function parseHelpTopic(value: string): HelpTopic | undefined {
  return HELP_TOPICS.find((topic) => topic === value);
}

/**
 * Where one topic lives in the UI: `/help` scrolled to its section. The
 * section's `id` is the topic itself, so the hash is the anchor.
 */
export function helpPath(topic: HelpTopic): string {
  return `/help#${topic}`;
}
