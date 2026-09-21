// The project's identity and its project-level actions, in one dense row
// (`SPEC.md`, "User-facing features", Projects; "Projects" table).
//
// `Fetch now` and `Retry clone` both answer the updated `Project`, so they
// write it straight into the page's query cache instead of asking for it
// again. `Delete` is two-step and names everything that goes with the project
// before it does; its 409 while a session is live is shown where the button
// is, without leaving the page.

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { useNavigate } from "react-router";
import { Alert, SubmitButton } from "../../components";
import {
  deleteProject,
  fetchProject,
  retryClone,
} from "../../services/projects";
import { queryKeys } from "../../services/queryKeys";
import type { Project } from "../../types";
import { formatRelative } from "../../utils/format";
import { ProjectStatusPill } from "../projects/ProjectStatusPill";
import { errorMessage, logUnexpected } from "../../services/errorMessage";

export interface ProjectHeaderProps {
  project: Project;
  settingsOpen: boolean;
  onToggleSettings: () => void;
}

const LABEL = "text-console-muted text-xs";
const VALUE = "text-console-text font-mono text-xs";

export function ProjectHeader({
  project,
  settingsOpen,
  onToggleSettings,
}: ProjectHeaderProps) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();

  const [error, setError] = useState<string | null>(null);
  const [confirmingDelete, setConfirmingDelete] = useState(false);

  const detailKey = queryKeys.projects.detail(project.id);

  /** Both refreshing actions answer the project; take it as the new truth. */
  function adopt(updated: Project) {
    queryClient.setQueryData(detailKey, updated);
    void queryClient.invalidateQueries({ queryKey: queryKeys.projects.list() });
    setError(null);
  }

  const fetchNow = useMutation({
    mutationFn: () => fetchProject(project.id),
    onSuccess: adopt,
    // An unreachable upstream leaves `last_fetched_at` as it was: nothing is
    // written to the cache, only the server's reason is shown.
    onError: (caught: unknown) => {
      logUnexpected(caught);
      setError(errorMessage(caught));
    },
  });

  const retry = useMutation({
    mutationFn: () => retryClone(project.id),
    onSuccess: adopt,
    onError: (caught: unknown) => {
      logUnexpected(caught);
      setError(errorMessage(caught));
    },
  });

  const remove = useMutation({
    mutationFn: () => deleteProject(project.id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.projects.all });
      void navigate("/projects");
    },
    onError: (caught: unknown) => {
      // 409 `refused while a session is running or being created`: stay here
      // so the user can go and stop the session.
      setConfirmingDelete(false);
      logUnexpected(caught);
      setError(errorMessage(caught));
    },
  });

  const busy = fetchNow.isPending || retry.isPending || remove.isPending;

  return (
    <section className="space-y-2">
      <div className="border-console-border bg-console-surface flex flex-wrap items-center gap-x-4 gap-y-2 rounded border px-3 py-2">
        <ProjectStatusPill status={project.status} />

        {/* The one thing on this strip that is a state rather than an
            identifier, and the only reason it takes colour: while it is set,
            nothing in the project launches itself (`ARCHITECTURE.md`, "Task
            tracker" → "Unattended launches"). */}
        {project.automation_paused && (
          <span className="border-state-parked/60 text-state-parked bg-console-surface inline-flex items-center gap-1.5 rounded border px-1.5 py-0.5 font-mono text-xs whitespace-nowrap">
            <span aria-hidden="true" className="size-1.5 rounded-full bg-current" />
            automation paused
          </span>
        )}

        <span className="min-w-0 truncate">
          <span className={LABEL}>remote </span>
          <span className={VALUE}>{project.remote_url}</span>
        </span>

        <span>
          <span className={LABEL}>branch </span>
          <span className={VALUE}>
            {project.default_branch ?? "discovering…"}
          </span>
        </span>

        <span>
          <span className={LABEL}>fetched </span>
          <span className={VALUE}>
            {formatRelative(project.last_fetched_at)}
          </span>
        </span>

        <span>
          <span className={LABEL}>max attempts </span>
          <span className={VALUE}>{project.max_attempts}</span>
        </span>

        <span>
          <span className={LABEL}>session cap </span>
          <span className={VALUE}>
            {project.max_concurrent_sessions ?? "none"}
          </span>
        </span>

        <div className="ml-auto flex shrink-0 items-center gap-2">
          <SubmitButton
            type="button"
            variant="ghost"
            loading={fetchNow.isPending}
            disabled={busy}
            onClick={() => {
              setError(null);
              fetchNow.mutate();
            }}
          >
            Fetch now
          </SubmitButton>

          {project.status === "error" && (
            <SubmitButton
              type="button"
              variant="primary"
              loading={retry.isPending}
              disabled={busy}
              onClick={() => {
                setError(null);
                retry.mutate();
              }}
            >
              Retry clone
            </SubmitButton>
          )}

          <SubmitButton
            type="button"
            variant="ghost"
            disabled={busy}
            onClick={onToggleSettings}
          >
            {settingsOpen ? "Close settings" : "Settings"}
          </SubmitButton>

          <SubmitButton
            type="button"
            variant="danger"
            disabled={busy}
            onClick={() => {
              setError(null);
              setConfirmingDelete(true);
            }}
          >
            Delete
          </SubmitButton>
        </div>
      </div>

      {project.status === "error" && project.status_message !== null && (
        <Alert kind="error">{project.status_message}</Alert>
      )}

      {error !== null && (
        <Alert
          kind="error"
          onDismiss={() => {
            setError(null);
          }}
        >
          {error}
        </Alert>
      )}

      {confirmingDelete && (
        <Alert kind="warning">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <span>
              Delete <span className="font-mono">{project.name}</span>? Its
              sessions, tasks, secrets, shared directories and git mirror are
              removed with it. This cannot be undone.
            </span>
            <span className="flex shrink-0 items-center gap-2">
              <SubmitButton
                type="button"
                variant="danger"
                loading={remove.isPending}
                onClick={() => {
                  remove.mutate();
                }}
              >
                Delete project
              </SubmitButton>
              <SubmitButton
                type="button"
                variant="ghost"
                disabled={remove.isPending}
                onClick={() => {
                  setConfirmingDelete(false);
                }}
              >
                Cancel
              </SubmitButton>
            </span>
          </div>
        </Alert>
      )}
    </section>
  );
}
