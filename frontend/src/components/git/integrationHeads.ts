// The integration heads of a project mirror, as the Branches tab lists them
// (`SPEC.md`, "Frontend", Routes; `help/branches.md`, "Three kinds of branch").
//
// Derived from `GET /projects/{id}/branches`, which already carries every ref
// the tab needs: the heads themselves and the upstream-tracking ref of the
// same name beside each. Nothing here counts commits — ahead/behind of a head
// against its upstream is not in `Branch` — so the upstream is shown as the
// commit it is at and nothing more.

import type { Branch } from "../../types";

export interface IntegrationHead {
  name: string;
  commit: string;
  /** This head is the project's `default_branch`. */
  isDefault: boolean;
  /** The commit of `origin/<name>`, or `null` when the remote has no such branch. */
  upstream: string | null;
}

/**
 * The heads of `branches`, the default one first and the rest by name, each
 * with the commit of its upstream-tracking ref when there is one.
 */
export function integrationHeads(
  branches: readonly Branch[],
  defaultBranch: string | null,
): IntegrationHead[] {
  const upstreams = new Map<string, string>(
    branches
      .filter((branch) => branch.kind === "upstream")
      .map((branch) => [branch.name, branch.commit]),
  );

  return branches
    .filter((branch) => branch.kind === "head")
    .map((branch) => ({
      name: branch.name,
      commit: branch.commit,
      isDefault: branch.name === defaultBranch,
      upstream: upstreams.get(`origin/${branch.name}`) ?? null,
    }))
    .sort((a, b) =>
      a.isDefault === b.isDefault
        ? a.name.localeCompare(b.name)
        : a.isDefault
          ? -1
          : 1,
    );
}
