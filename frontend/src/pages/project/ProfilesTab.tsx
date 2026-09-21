// The profiles tab of `/projects/:id` (`SPEC.md`, "Agent profiles"): the
// project's agent profiles and the editor that creates and replaces one.
//
// The editor is not a route of its own; it is this panel in another mode,
// selected by `?profile=new|<id>` alongside `?tab=profiles`. That keeps the
// project shell — header, settings, tab strip — on screen while a profile is
// being written, and still gives an open editor a link.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useSearchParams } from "react-router";
import {
  Alert,
  EmptyState,
  LoadingState,
  SectionHeader,
  SubmitButton,
} from "../../components";
import { deleteProfile, listProfiles } from "../../services/profiles";
import { queryKeys } from "../../services/queryKeys";
import type { Profile } from "../../types";
import { ProfileEditorPage } from "../ProfileEditorPage";
import { projectErrorMessage } from "./messages";
import type { ProjectTabPanelProps } from "./tabs";

const HEAD = "text-console-muted py-1.5 pr-3 text-left text-xs font-normal";
const CELL = "py-1.5 pr-3 align-top";

/** The image a new profile starts on: the one the project's default uses. */
function defaultImageOf(profiles: Profile[]): string {
  return profiles.find((profile) => profile.is_default)?.image ?? "";
}

export function ProfilesTab({ project }: ProjectTabPanelProps) {
  const [search, setSearch] = useSearchParams();
  const selected = search.get("profile");

  const profiles = useQuery({
    queryKey: queryKeys.projects.profiles(project.id),
    queryFn: () => listProfiles(project.id),
    retry: false,
  });

  /** `null` closes the editor; `"new"` or an id opens it. */
  function openEditor(profile: string | null) {
    const next = new URLSearchParams(search);
    next.set("tab", "profiles");
    if (profile === null) {
      next.delete("profile");
    } else {
      next.set("profile", profile);
    }
    setSearch(next);
  }

  if (profiles.isPending) {
    return <LoadingState label="Loading profiles" />;
  }

  if (profiles.isError) {
    return (
      <Alert kind="error">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <span>{projectErrorMessage(profiles.error)}</span>
          <SubmitButton
            type="button"
            variant="ghost"
            loading={profiles.isFetching}
            onClick={() => {
              void profiles.refetch();
            }}
          >
            Try again
          </SubmitButton>
        </div>
      </Alert>
    );
  }

  const rows = profiles.data;

  if (selected !== null) {
    const editing = selected === "new" ? null : rows.find((p) => p.id === selected);

    // The id in the URL names no profile of this project — deleted in another
    // tab, or simply wrong. Say so rather than opening an empty create form.
    if (editing === undefined) {
      return (
        <Alert kind="error">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span>That profile no longer exists.</span>
            <SubmitButton
              type="button"
              variant="ghost"
              loading={false}
              onClick={() => {
                openEditor(null);
              }}
            >
              Back to profiles
            </SubmitButton>
          </div>
        </Alert>
      );
    }

    return (
      <ProfileEditorPage
        // Remounts when the editor moves to another profile, so the form
        // starts from that profile's values instead of the previous one's.
        key={selected}
        projectId={project.id}
        profile={editing}
        defaultImage={defaultImageOf(rows)}
        // For the `<name>-2` suffix a role template's name takes when the
        // project already has that role (`SPEC.md`, "Frontend").
        existingNames={rows.map((profile) => profile.name)}
        onClose={() => {
          openEditor(null);
        }}
      />
    );
  }

  const newProfile = (
    <SubmitButton
      type="button"
      loading={false}
      onClick={() => {
        openEditor("new");
      }}
    >
      New profile
    </SubmitButton>
  );

  return (
    <section className="space-y-3">
      <SectionHeader
        title="Agent profiles"
        description="What an agent is: its image, its prompt, the states it serves and what it may reach."
        actions={newProfile}
      />

      {rows.length === 0 ? (
        <EmptyState
          title="No profiles yet"
          description="A profile decides which image a session runs in and which task states its agent picks work up from."
          action={newProfile}
        />
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full border-collapse text-sm">
            <thead>
              <tr className="border-console-border border-b">
                <th scope="col" className={HEAD}>
                  Name
                </th>
                <th scope="col" className={HEAD}>
                  Kind
                </th>
                <th scope="col" className={HEAD}>
                  Model
                </th>
                <th scope="col" className={`${HEAD} hidden lg:table-cell`}>
                  Image
                </th>
                <th scope="col" className={HEAD}>
                  Serves
                </th>
                <th scope="col" className={`${HEAD} hidden md:table-cell`}>
                  Idle timeout
                </th>
                <th scope="col" className={`${HEAD} pr-0 text-right`}>
                  Actions
                </th>
              </tr>
            </thead>
            <tbody>
              {rows.map((profile) => (
                <ProfileRow
                  key={profile.id}
                  projectId={project.id}
                  profile={profile}
                  onEdit={() => {
                    openEditor(profile.id);
                  }}
                />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

interface ProfileRowProps {
  projectId: string;
  profile: Profile;
  onEdit: () => void;
}

function ProfileRow({ projectId, profile, onEdit }: ProfileRowProps) {
  const queryClient = useQueryClient();

  const remove = useMutation({
    mutationFn: () => deleteProfile(projectId, profile.id),
    onSuccess: () => {
      void queryClient.invalidateQueries({
        queryKey: queryKeys.projects.profiles(projectId),
      });
    },
  });

  function onDelete() {
    if (
      !window.confirm(
        `Delete ${profile.name}? Sessions already running keep their configuration.`,
      )
    ) {
      return;
    }
    remove.mutate();
  }

  return (
    <>
      <tr className="border-console-border/60 border-b last:border-b-0">
        <td className={`${CELL} font-mono text-xs`}>
          <span className="text-console-text">{profile.name}</span>
          {profile.is_default && (
            <span className="border-console-border text-console-muted ml-2 rounded border px-1.5 py-0.5">
              default
            </span>
          )}
        </td>

        <td className={`${CELL} text-console-muted font-mono text-xs`}>
          {profile.kind}
        </td>

        <td className={`${CELL} text-console-muted font-mono text-xs`}>
          {profile.model ?? "CLI default"}
        </td>

        <td
          className={`${CELL} text-console-muted hidden font-mono text-xs break-all lg:table-cell`}
        >
          {profile.image}
        </td>

        <td className={CELL}>
          <div className="flex flex-wrap gap-1">
            {profile.serves_states.length === 0 ? (
              <span className="text-console-muted text-xs">nothing</span>
            ) : (
              profile.serves_states.map((state) => (
                <span
                  key={state}
                  className="border-console-border text-console-muted rounded border px-1.5 py-0.5 font-mono text-xs"
                >
                  {state}
                </span>
              ))
            )}
          </div>
        </td>

        <td
          className={`${CELL} text-console-muted hidden font-mono text-xs whitespace-nowrap md:table-cell`}
        >
          {String(profile.idle_timeout_secs)}s
        </td>

        <td className={`${CELL} pr-0`}>
          <div className="flex flex-wrap justify-end gap-1.5">
            <SubmitButton
              type="button"
              variant="ghost"
              loading={false}
              disabled={remove.isPending}
              onClick={onEdit}
            >
              Edit
            </SubmitButton>
            {/* A project always keeps one default profile, so the API refuses
                to delete it; the button says so before the request. */}
            <span title={profile.is_default ? "default profile" : undefined}>
              <SubmitButton
                type="button"
                variant="danger"
                loading={remove.isPending}
                disabled={profile.is_default}
                onClick={onDelete}
              >
                Delete
              </SubmitButton>
            </span>
          </div>
        </td>
      </tr>

      {remove.isError && (
        <tr className="border-console-border/60 border-b last:border-b-0">
          <td colSpan={7} className="px-0 py-2">
            <Alert kind="error">{projectErrorMessage(remove.error)}</Alert>
          </td>
        </tr>
      )}
    </>
  );
}
