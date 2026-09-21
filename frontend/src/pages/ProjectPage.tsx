// `/projects/:id` and `/projects/:id/tasks/:number` (`SPEC.md`, "Frontend",
// Routes): the project shell — the header with its project-level actions, the
// collapsed settings form, and the tab strip over one panel.
//
// The shell owns the single read of the project (`useProject`) and hands the
// loaded `Project` to whichever panel is showing, so switching tabs is free.
// The tab is the `?tab=` search parameter; the `/tasks/:number` route is the
// board with a task open, so it forces the board tab and leaves the drawer to
// the board itself.

import { useState } from "react";
import { useParams, useSearchParams } from "react-router";
import { LoadingState } from "../components/LoadingState";
import { PageLayout } from "../components/PageLayout";
import { QueryErrorAlert } from "../components/QueryErrorAlert";
import { SecretsManager } from "../components/secrets/SecretsManager";
import { errorMessage, isNotFound } from "../services/errorMessage";
import { TaskStatesEditor } from "../tasks/TaskStatesEditor";
import type { Project } from "../types";
import { isUuid } from "../utils/uuid";
import { NotFoundPage } from "./NotFoundPage";
import {
  BoardTab,
  ProfilesTab,
  ProjectHeader,
  ProjectSettingsForm,
  ProjectTabs,
  SessionsTab,
  parseProjectTab,
  SharedDirsTab,
  useProject,
} from "./project";
import type { ProjectTab } from "./project";

export function ProjectPage() {
  const params = useParams();
  const [search] = useSearchParams();
  const [settingsOpen, setSettingsOpen] = useState(false);

  const id = params.id;
  // `/projects/:id/tasks/:number` is the board with that task's drawer open.
  const tab: ProjectTab =
    params.number === undefined ? parseProjectTab(search.get("tab")) : "board";
  // The parameter travels as it was written: the board panel decides what it
  // names, because "no task in the URL" and "a task number that is not one"
  // are different answers — no drawer, and a drawer that says not found.
  const taskParam = params.number;

  // An id that is not a UUID names no project: answer without asking.
  if (!isUuid(id)) {
    return <NotFoundPage />;
  }

  return (
    // The project's id is this page's identity boundary, as `task.id` is the
    // task drawer's: `ProjectHeader`'s delete confirmation, the settings
    // form's fields and every other piece of state below belongs to the
    // project it was opened on, and a cached project renders with no loading
    // state in between to unmount them. `key` makes another project a
    // remount, while a refetch of the same one leaves an open form alone.
    <ProjectView
      key={id}
      id={id}
      tab={tab}
      taskParam={taskParam}
      settingsOpen={settingsOpen}
      onToggleSettings={() => {
        setSettingsOpen((open) => !open);
      }}
    />
  );
}

interface ProjectViewProps {
  id: string;
  tab: ProjectTab;
  /** The `:number` of `/projects/:id/tasks/:number`, for the board panel. */
  taskParam: string | undefined;
  settingsOpen: boolean;
  onToggleSettings: () => void;
}

/**
 * Split from `ProjectPage` so the not-found shortcut above can return before
 * any hook of the loaded page runs.
 */
function ProjectView({
  id,
  tab,
  taskParam,
  settingsOpen,
  onToggleSettings,
}: ProjectViewProps) {
  const project = useProject(id);

  // The project stopped existing — deleted here or in another tab — whether or
  // not this page had already loaded it.
  if (project.isError && isNotFound(project.error)) {
    return <NotFoundPage />;
  }

  const data = project.data;

  // Nothing to show yet: the first read is still out, or it failed.
  if (data === undefined) {
    return (
      <PageLayout title="Project">
        {project.isPending ? (
          <LoadingState label="Loading project" />
        ) : (
          <QueryErrorAlert
            query={project}
            message={errorMessage(project.error)}
          />
        )}
      </PageLayout>
    );
  }

  return (
    <PageLayout title={data.name}>
      <div className="space-y-4">
        {/* A failed refetch — the focus refresh or the cloning poll meeting a
            restarting orchestrator — is a banner, never an unmount: the
            settings form, the open editor and the board keep what is in them
            (`SPEC.md`, "Frontend", Read failures). */}
        {project.isError && (
          <QueryErrorAlert
            query={project}
            message={errorMessage(project.error)}
          />
        )}

        <ProjectHeader
          project={data}
          settingsOpen={settingsOpen}
          onToggleSettings={onToggleSettings}
        />

        {settingsOpen && <ProjectSettingsForm project={data} />}

        <ProjectTabs projectId={data.id} active={tab} />

        <ProjectPanel project={data} tab={tab} taskParam={taskParam} />
      </div>
    </PageLayout>
  );
}

function ProjectPanel({
  project,
  tab,
  taskParam,
}: {
  project: Project;
  tab: ProjectTab;
  taskParam: string | undefined;
}) {
  // Nothing under the tabs exists until the mirror does; the header and the
  // settings form stay usable and the page polls until the clone settles.
  if (project.status === "cloning") {
    return <LoadingState label="Clone in progress" />;
  }

  if (tab === "sessions") {
    return <SessionsTab project={project} />;
  }

  if (tab === "profiles") {
    return <ProfilesTab project={project} />;
  }

  if (tab === "secrets") {
    return (
      <SecretsManager
        scope="project"
        scopeId={project.id}
        title="Project secrets"
      />
    );
  }

  if (tab === "shared-dirs") {
    return <SharedDirsTab project={project} />;
  }

  if (tab === "states") {
    return <TaskStatesEditor projectId={project.id} />;
  }

  // `board`, which is also where `/projects/:id/tasks/:number` lands: the one
  // place the task stream is mounted.
  return <BoardTab project={project} taskParam={taskParam} />;
}
