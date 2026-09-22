// `SPEC.md`, "Frontend", Routes: `/projects`. Every user sees every project
// (no per-project authorisation in v1), so this is the whole inventory in one
// table: what each project is, where it came from, and how its clone is
// getting on.
//
// `POST /projects` returns as soon as the row exists, with `status: cloning`
// (`SPEC.md`, "User-facing features"). The clone finishes in the background,
// so the list polls every three seconds for as long as any project is still
// cloning and stops the moment none is — and never polls a hidden tab
// (`refetchIntervalInBackground: false`).
//
// The page reads the list; a row's retry, its pending state and its refusal
// are `ProjectRow`'s own, so two retries in flight keep two rows busy.

import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { EmptyState } from "../components/EmptyState";
import { LoadingState } from "../components/LoadingState";
import { PageLayout } from "../components/PageLayout";
import { QueryErrorAlert } from "../components/QueryErrorAlert";
import { SubmitButton } from "../components/SubmitButton";
import { TableHead } from "../components/TableHead";
import { TABLE, type TableColumn } from "../components/tableStyles";
import { errorMessage } from "../services/errorMessage";
import { listProjects } from "../services/projects";
import { queryKeys } from "../services/queryKeys";
import type { Project } from "../types";
import { ProjectCreateForm } from "./projects/ProjectCreateForm";
import { ProjectRow } from "./projects/ProjectRow";

/** How often a project that is still cloning is asked about. */
const PROJECTS_REFETCH_MS = 3_000;

const COLUMNS: readonly TableColumn[] = [
  { label: "Status" },
  { label: "Project" },
  { label: "Remote", className: "hidden md:table-cell" },
  { label: "Branch", className: "hidden sm:table-cell" },
  { label: "Fetched", className: "pr-0 text-right" },
];

/**
 * Work first: anything cloning or failed is what the operator came for, then
 * the settled projects by name. Names are unique, so the order is total and
 * a row never moves under the cursor between two polls.
 */
function byAttentionThenName(a: Project, b: Project): number {
  const rank = (p: Project) => (p.status === "ready" ? 1 : 0);
  return rank(a) - rank(b) || a.name.localeCompare(b.name);
}

export function ProjectsPage() {
  const [creating, setCreating] = useState(false);

  const projects = useQuery({
    queryKey: queryKeys.projects.list(),
    queryFn: listProjects,
    refetchInterval: (query) =>
      (query.state.data ?? []).some((project) => project.status === "cloning")
        ? PROJECTS_REFETCH_MS
        : false,
    refetchIntervalInBackground: false,
    placeholderData: keepPreviousData,
  });

  const rows = useMemo(
    () => [...(projects.data ?? [])].sort(byAttentionThenName),
    [projects.data],
  );

  const newProjectButton = (
    <SubmitButton
      type="button"
      disabled={creating}
      onClick={() => {
        setCreating(true);
      }}
    >
      New project
    </SubmitButton>
  );

  return (
    <PageLayout title="Projects" actions={newProjectButton}>
      <div className="space-y-4">
        {creating && (
          <ProjectCreateForm
            onCancel={() => {
              setCreating(false);
            }}
          />
        )}

        {projects.isError && (
          <QueryErrorAlert
            query={projects}
            message={`Could not load projects. ${errorMessage(projects.error)}`}
          />
        )}

        {/* A read that failed says nothing about how many projects there are,
            so it never becomes "No projects yet" under its own alert
            (`SPEC.md`, "Frontend", Read failures). */}
        {projects.isPending ? (
          <LoadingState label="Loading projects" />
        ) : rows.length === 0 ? (
          projects.isSuccess && (
            <EmptyState
              title="No projects yet"
              description="A project is a git remote Mars mirrors once and then runs every session from; its sessions also need an agent credential."
              help="getting-started"
              action={newProjectButton}
            />
          )
        ) : (
          <table className={TABLE}>
            <TableHead columns={COLUMNS} />
            <tbody>
              {rows.map((project) => (
                <ProjectRow key={project.id} project={project} />
              ))}
            </tbody>
          </table>
        )}
      </div>
    </PageLayout>
  );
}
