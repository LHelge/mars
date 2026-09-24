// The `branches` tab of `/projects/:id` (`SPEC.md`, "Frontend", Routes): the
// project's git work — its integration heads, bringing upstream into them and
// the session refs waiting to be merged — which is project work rather than
// session work, so it has a tab of its own and not the bottom of the Sessions
// tab. It is the project form of `GitActionsPanel`, the same component the
// session view shows filtered to one row behind its header's `branch` toggle.

import { GitActionsPanel } from "../../components/git/GitActionsPanel";
import type { ProjectTabPanelProps } from "./tabs";

export function BranchesTab({ project }: ProjectTabPanelProps) {
  return <GitActionsPanel project={project} />;
}
