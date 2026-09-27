// One row of the projects table, and the one action it offers: retrying a
// clone that failed (`POST /projects/{id}/retry-clone`).
//
// The mutation is the row's own, as it is in the administration tables: a
// table-wide observer follows only its latest `mutate()`, so retrying a second
// failed project would take the first row's spinner away while its request is
// still in flight. Its refusal is read straight off the mutation and rendered
// inside the failed-clone block, so a row that has moved on to `cloning` or
// `ready` cannot still be showing why a retry was refused a minute ago
// (`CLAUDE.md`, "Frontend conventions", Submitting a form).

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Link } from "react-router";
import { Icon, ICON_CLASS } from "../../components/icons";
import { SubmitButton } from "../../components/SubmitButton";
import { errorMessage, logUnexpected } from "../../services/errorMessage";
import { adoptProject } from "../../services/projectCache";
import { retryClone } from "../../services/projects";
import { queryKeys } from "../../services/queryKeys";
import type { Project } from "../../types";
import { formatRelative, PLACEHOLDER } from "../../utils/format";
import { CELL_TOP as CELL, ROW } from "../../components/tableStyles";
import { ProjectStatusPill } from "./ProjectStatusPill";

export interface ProjectRowProps {
  project: Project;
}

export function ProjectRow({ project }: ProjectRowProps) {
  const queryClient = useQueryClient();

  const retry = useMutation({
    mutationFn: () => retryClone(project.id),
    // The same answer the project page takes from its own `Retry clone`
    // (`services/projectCache.ts`).
    onSuccess: (updated) => {
      adoptProject(queryClient, updated);
    },
    onError: (caught: unknown) => {
      logUnexpected(caught);
      // The project may have moved on since the list was read; find out.
      void queryClient.invalidateQueries({
        queryKey: queryKeys.projects.list(),
      });
    },
  });

  return (
    <tr className={ROW}>
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
                loading={retry.isPending}
                onClick={() => {
                  retry.mutate();
                }}
              >
                Retry clone
              </SubmitButton>
              {retry.error !== null && (
                <span
                  role="alert"
                  className="text-state-failed max-w-[36ch] text-xs"
                >
                  {errorMessage(retry.error, "Could not retry the clone.")}
                </span>
              )}
            </>
          )}
        </div>
      </td>

      {/* Below `md` this cell carries the remote and takes the rest of the
          row: `w-full max-w-0` is what holds a `<td>` to a width, so the
          remote under the name truncates instead of widening the column to
          the whole URL (`components/tableStyles.ts`). */}
      <td className={`${CELL} max-md:w-full max-md:max-w-0`}>
        <div className="flex items-center gap-1.5">
          <Link
            to={`/projects/${project.id}`}
            className="text-console-text hover:text-console-accent min-w-0 wrap-anywhere"
          >
            {project.name}
          </Link>
          {project.has_credential && (
            <Icon.lock
              role="img"
              aria-label="Credential stored"
              className={`text-console-muted ${ICON_CLASS}`}
            />
          )}
        </div>
        {/* Below `md` the remote rides under the name rather than disappearing
            with its column. */}
        <span
          className="text-console-muted block truncate font-mono text-xs md:hidden"
          title={project.remote_url}
        >
          {project.remote_url}
        </span>
      </td>

      <td
        className={`${CELL} text-console-muted hidden font-mono text-xs md:table-cell`}
      >
        {/* The cap is on the block that truncates: a `max-w` on the `<td>`
            is ignored by automatic table layout. */}
        <span
          className="block max-w-[44ch] truncate"
          title={project.remote_url}
        >
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
}
