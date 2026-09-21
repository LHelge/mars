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

import {
  keepPreviousData,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { LockClosedIcon } from "@heroicons/react/24/outline";
import { useMemo, useState } from "react";
import { Link } from "react-router";
import {
  EmptyState,
  LoadingState,
  PageLayout,
  QueryErrorAlert,
  SubmitButton,
} from "../components";
import { errorMessage, logUnexpected } from "../services/errorMessage";
import { listProjects, retryClone } from "../services/projects";
import { queryKeys } from "../services/queryKeys";
import type { Project } from "../types";
import { formatRelative, PLACEHOLDER } from "../utils/format";
import { ProjectCreateForm } from "./projects/ProjectCreateForm";
import { ProjectStatusPill } from "./projects/ProjectStatusPill";

/** How often a project that is still cloning is asked about. */
export const PROJECTS_REFETCH_MS = 3_000;

const CELL = "py-2 pr-3 align-top";
const HEAD = "text-console-muted py-1.5 pr-3 text-left text-xs font-normal";
const ROW = "border-console-border/60 border-b last:border-b-0";

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
  const queryClient = useQueryClient();
  const [creating, setCreating] = useState(false);
  // One message per project id: a retry that came back 409 explains itself on
  // the row it was pressed on.
  const [retryErrors, setRetryErrors] = useState<Record<string, string>>({});

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

  const retry = useMutation({
    mutationFn: (id: string) => retryClone(id),
    onSuccess: (project) => {
      setRetryErrors((current) => {
        const next = { ...current };
        delete next[project.id];
        return next;
      });
      queryClient.setQueryData(queryKeys.projects.detail(project.id), project);
      void queryClient.invalidateQueries({ queryKey: queryKeys.projects.all });
    },
    onError: (caught: unknown, id) => {
      logUnexpected(caught);
      const message = errorMessage(caught, "Could not retry the clone.");
      setRetryErrors((current) => ({ ...current, [id]: message }));
      // The project may have moved on since the list was read; find out.
      void queryClient.invalidateQueries({ queryKey: queryKeys.projects.all });
    },
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
              description="A project is a git remote Mars mirrors once and then runs every session from."
              action={newProjectButton}
            />
          )
        ) : (
          <table className="w-full border-collapse text-sm">
            <thead>
              <tr className="border-console-border border-b">
                <th scope="col" className={HEAD}>
                  Status
                </th>
                <th scope="col" className={HEAD}>
                  Project
                </th>
                <th scope="col" className={`${HEAD} hidden md:table-cell`}>
                  Remote
                </th>
                <th scope="col" className={`${HEAD} hidden sm:table-cell`}>
                  Branch
                </th>
                <th scope="col" className={`${HEAD} pr-0 text-right`}>
                  Fetched
                </th>
              </tr>
            </thead>
            <tbody>
              {rows.map((project) => {
                const retryError = retryErrors[project.id];
                const retrying =
                  retry.isPending && retry.variables === project.id;

                return (
                  <tr key={project.id} className={ROW}>
                    <td className={CELL}>
                      <div className="flex flex-col items-start gap-1">
                        <ProjectStatusPill status={project.status} />
                        {project.status === "error" && (
                          <>
                            <span className="text-state-failed max-w-[36ch] text-xs">
                              {project.status_message ?? "The clone failed."}
                            </span>
                            <SubmitButton
                              type="button"
                              variant="ghost"
                              loading={retrying}
                              onClick={() => {
                                retry.mutate(project.id);
                              }}
                            >
                              Retry clone
                            </SubmitButton>
                          </>
                        )}
                        {retryError !== undefined && (
                          <span
                            role="alert"
                            className="text-state-failed max-w-[36ch] text-xs"
                          >
                            {retryError}
                          </span>
                        )}
                      </div>
                    </td>

                    <td className={`${CELL} min-w-0`}>
                      <div className="flex items-center gap-1.5">
                        <Link
                          to={`/projects/${project.id}`}
                          className="text-console-text hover:text-console-accent"
                        >
                          {project.name}
                        </Link>
                        {project.has_credential && (
                          <LockClosedIcon
                            className="text-console-muted size-3.5 shrink-0"
                            aria-label="Credential stored"
                          />
                        )}
                      </div>
                      {/* Below `md` the remote rides under the name rather
                          than disappearing with its column. */}
                      <span className="text-console-muted block truncate font-mono text-xs md:hidden">
                        {project.remote_url}
                      </span>
                    </td>

                    <td
                      className={`${CELL} text-console-muted hidden max-w-[44ch] font-mono text-xs md:table-cell`}
                    >
                      <span className="block truncate" title={project.remote_url}>
                        {project.remote_url}
                      </span>
                    </td>

                    <td
                      className={`${CELL} text-console-muted hidden font-mono text-xs sm:table-cell`}
                    >
                      {project.default_branch ?? "discovering…"}
                    </td>

                    <td
                      className={`${CELL} text-console-muted pr-0 text-right font-mono text-xs whitespace-nowrap`}
                    >
                      {project.last_fetched_at === null
                        ? PLACEHOLDER
                        : formatRelative(project.last_fetched_at)}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </div>
    </PageLayout>
  );
}
