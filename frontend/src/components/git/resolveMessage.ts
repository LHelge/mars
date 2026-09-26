// What "Resolve with an agent" on a conflicting merge sends (`SPEC.md`,
// "Frontend", the git panel's conflict answer): the source described the way
// the `resolver` role template expects its first message to name it
// (`SPEC.md`, "Role profile templates" → `resolver`), and which profile the
// form preselects.
//
// Pure, so the message and the preselection are tested without a form.

import type { Branch, Profile } from "../../types";
import { shortSha } from "../../utils/format";

/**
 * The source of the merge that conflicted, by what the resolver fetches: a
 * session ref, a hand-off ref, an integration head (an ordinary branch of the
 * clone's `origin`) or an upstream-tracking ref of the mirror.
 */
export type ResolveSource =
  | { kind: "session"; sessionId: string }
  | { kind: "handoff"; handoffId: string }
  | { kind: "head"; name: string }
  | { kind: "upstream"; name: string };

/** The merge that conflicted, and what the panel knows of its two commits. */
export interface ResolveMerge {
  source: ResolveSource;
  /** The full commit of the source, or `null` when the panel cannot name it. */
  sourceCommit: string | null;
  /** The integration head the merge went into, which the session starts from. */
  target: string;
  /** The target's commit, or `null` when the listing has not got it. */
  targetCommit: string | null;
  paths: readonly string[];
}

/** How the merge API and the listings name the source. */
export function sourceName(source: ResolveSource): string {
  switch (source.kind) {
    case "session":
      return `refs/sessions/${source.sessionId}`;
    case "handoff":
      return `refs/handoffs/${source.handoffId}`;
    case "head":
    case "upstream":
      return source.name;
  }
}

/**
 * What the resolver's clone fetches the source with. Its `origin` is the
 * project repository, whose session, hand-off and upstream-tracking refs an
 * ordinary fetch does not copy (`ARCHITECTURE.md`, "Git model", "Session
 * clone"), so all three are spelled out; an integration head is a branch of
 * `origin` and its name is enough.
 */
export function sourceRefspec(source: ResolveSource): string {
  switch (source.kind) {
    case "session":
    case "handoff":
      return sourceName(source);
    case "head":
      return source.name;
    case "upstream":
      return `refs/remotes/${source.name}`;
  }
}

/**
 * The source a merge named, from the ref listing when it has the name and from
 * the name's own spelling when it has not: the API names upstream-tracking
 * refs `origin/<branch>` and session refs `refs/sessions/<id>`
 * (`SPEC.md`, "Projects", `Branch`).
 */
export function sourceOfRef(
  name: string,
  branches: readonly Branch[],
): ResolveSource {
  const listed = branches.find((branch) => branch.name === name);
  const sessionPrefix = "refs/sessions/";
  if (listed?.kind === "session" && listed.session_id !== undefined) {
    return { kind: "session", sessionId: listed.session_id };
  }
  if (name.startsWith(sessionPrefix)) {
    return { kind: "session", sessionId: name.slice(sessionPrefix.length) };
  }
  if (listed?.kind === "upstream" || name.startsWith("origin/")) {
    return { kind: "upstream", name };
  }
  return { kind: "head", name };
}

/** The listing's commit for a source, or `null` when it does not list it. */
export function commitOfSource(
  source: ResolveSource,
  branches: readonly Branch[],
): string | null {
  const name = sourceName(source);
  const listed = branches.find(
    (branch) =>
      branch.name === name ||
      (source.kind === "session" &&
        branch.kind === "session" &&
        branch.session_id === source.sessionId),
  );
  return listed?.commit ?? null;
}

/**
 * The generated first message. A commit the panel cannot name is left out
 * rather than guessed, and the resolver then has nothing to check the fetched
 * tip against but the ref itself.
 */
export function resolveMessage(merge: ResolveMerge): string {
  const source =
    merge.sourceCommit === null
      ? sourceName(merge.source)
      : `${sourceName(merge.source)} (${merge.sourceCommit})`;
  const target =
    merge.targetCommit === null
      ? merge.target
      : `${merge.target} (${shortSha(merge.targetCommit)})`;
  return [
    `Merge ${source} into your branch, which starts at ${target}.`,
    "The merge Mars tried conflicted in:",
    ...merge.paths.map((path) => `- ${path}`),
    `Fetch the source with: git fetch origin ${sourceRefspec(merge.source)}`,
    `Resolve the conflicts, run the project's checks, commit the merge, and tell me when your branch is ready to merge into ${merge.target}. Do not merge into ${merge.target} yourself.`,
  ].join("\n");
}

/** The name of the role template this action is made for. */
export const RESOLVER_PROFILE = "resolver";

/** The profiles the action may launch: a resolution is a conversation. */
export function conversationalProfiles(
  profiles: readonly Profile[],
): Profile[] {
  return profiles.filter((profile) => profile.kind === "conversational");
}

/**
 * The profile the form has selected: the one the user chose, else one named
 * `resolver`, else the project's default, else the first offered — among the
 * conversational `offered` alone.
 */
export function selectedResolver(
  offered: readonly Profile[],
  chosenId: string,
): Profile | undefined {
  return (
    offered.find((profile) => profile.id === chosenId) ??
    offered.find((profile) => profile.name === RESOLVER_PROFILE) ??
    offered.find((profile) => profile.is_default) ??
    offered[0]
  );
}
