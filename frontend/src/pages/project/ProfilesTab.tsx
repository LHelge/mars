// The profiles tab of `/projects/:id` (`SPEC.md`, "Agent profiles"): the
// project's agent profiles and the editor that creates and replaces one.
//
// The editor is not a route of its own; it is this panel in another mode,
// selected by `?profile=new|<id>` alongside `?tab=profiles`. That keeps the
// project shell — header, settings, tab strip — on screen while a profile is
// being written, and still gives an open editor a link.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { useSearchParams } from "react-router";
import { Alert } from "../../components/Alert";
import { EmptyState } from "../../components/EmptyState";
import { LoadingState } from "../../components/LoadingState";
import { QueryErrorAlert } from "../../components/QueryErrorAlert";
import { SectionHeader } from "../../components/SectionHeader";
import { ConfirmPanel } from "../../components/ConfirmPanel";
import { SubmitButton } from "../../components/SubmitButton";
import { TableHead } from "../../components/TableHead";
import {
  CELL_TOP,
  ROW,
  SPAN_CELL_BARE,
  TABLE,
  X_SCROLLER,
  type TableColumn,
} from "../../components/tableStyles";
import { deleteProfile } from "../../services/profiles";
import { queryKeys } from "../../services/queryKeys";
import { projectQueries } from "../../services/queryOptions";
import type { Profile } from "../../types";
import { ProfileEditor } from "./ProfileEditor";
import { errorMessage } from "../../services/errorMessage";
import { formatUtc, PLACEHOLDER } from "../../utils/format";
import { PROFILE_AUTOMATION } from "../../utils/testIds";
import type { ProjectTabPanelProps } from "./tabs";

const COLUMNS: readonly TableColumn[] = [
  { label: "Name" },
  { label: "Kind", className: "hidden sm:table-cell" },
  { label: "Model", className: "hidden sm:table-cell" },
  { label: "Image", className: "hidden lg:table-cell" },
  { label: "Serves", className: "hidden sm:table-cell" },
  { label: "Automation" },
  { label: "Schedule", className: "hidden lg:table-cell" },
  { label: "Idle timeout", className: "hidden md:table-cell" },
  { label: "Actions", className: "pr-0 text-right" },
];

/**
 * One automation marker. Provenance-like and not a state, so it takes no
 * colour: the same outlined monospace chip the served states use.
 */
const AUTOMATION_TAG =
  "border-console-border text-console-muted rounded border px-1.5 py-0.5 font-mono text-xs";

/** The image a new profile starts on: the one the project's default uses. */
function defaultImageOf(profiles: Profile[]): string {
  return profiles.find((profile) => profile.is_default)?.image ?? "";
}

export function ProfilesTab({ project }: ProjectTabPanelProps) {
  const [search, setSearch] = useSearchParams();
  const selected = search.get("profile");

  const profiles = useQuery(projectQueries.profiles(project.id));

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

  const rows = profiles.data;

  // The list has never arrived: still on its way, or the first read failed.
  if (rows === undefined) {
    return profiles.isPending ? (
      <LoadingState label="Loading profiles" />
    ) : (
      <QueryErrorAlert
        query={profiles}
        message={errorMessage(profiles.error)}
      />
    );
  }

  // A refetch that failed over a list already on screen — and over an open
  // editor with a system prompt half written in it — is a banner and nothing
  // more (`SPEC.md`, "Frontend", Read failures).
  const staleWarning = profiles.isError ? (
    <QueryErrorAlert query={profiles} message={errorMessage(profiles.error)} />
  ) : null;

  if (selected !== null) {
    const editing =
      selected === "new" ? null : rows.find((p) => p.id === selected);

    // The id in the URL names no profile of this project — deleted in another
    // tab, or simply wrong. Say so rather than opening an empty create form.
    if (editing === undefined) {
      return (
        <QueryErrorAlert
          message="That profile no longer exists."
          retryLabel="Back to profiles"
          onRetry={() => {
            openEditor(null);
          }}
        />
      );
    }

    return (
      <div className="space-y-3">
        {staleWarning}
        <ProfileEditor
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
      </div>
    );
  }

  const newProfile = (
    <SubmitButton
      type="button"
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
        help="profiles"
        actions={newProfile}
      />

      {staleWarning}

      {rows.length === 0 ? (
        <EmptyState
          title="No profiles yet"
          description="A profile decides which image a session runs in and which task states its agent picks work up from."
          help="profiles"
          action={newProfile}
        />
      ) : (
        <div className={X_SCROLLER}>
          <table className={TABLE}>
            <TableHead columns={COLUMNS} />
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
  const [confirming, setConfirming] = useState(false);
  const defaultNoteId = `profile-${profile.id}-default-note`;

  const remove = useMutation({
    mutationFn: () => deleteProfile(projectId, profile.id),
    onSuccess: () => {
      void queryClient.invalidateQueries({
        queryKey: queryKeys.projects.profiles(projectId),
      });
    },
  });

  function onDelete() {
    setConfirming(false);
    remove.mutate();
  }

  return (
    <>
      <tr className={ROW}>
        <td className={`${CELL_TOP} font-mono text-xs`}>
          <span className="text-console-text">{profile.name}</span>
          {profile.is_default && (
            <>
              <span className="border-console-border text-console-muted ml-2 rounded border px-1.5 py-0.5">
                default
              </span>
              {/* What the chip means, said rather than hovered: the launch
                  form starts on it, and the API refuses to delete it. */}
              <span
                id={defaultNoteId}
                className="text-console-muted block pt-1 font-sans"
              >
                Pre-selected in the project&rsquo;s launch form; cannot be
                deleted.
              </span>
            </>
          )}
        </td>

        <td
          className={`${CELL_TOP} text-console-muted hidden font-mono text-xs sm:table-cell`}
        >
          {profile.kind}
        </td>

        <td
          className={`${CELL_TOP} text-console-muted hidden font-mono text-xs sm:table-cell`}
        >
          {profile.model ?? "CLI default"}
        </td>

        <td
          className={`${CELL_TOP} text-console-muted hidden font-mono text-xs break-all lg:table-cell`}
        >
          {profile.image}
        </td>

        <td className={`${CELL_TOP} hidden sm:table-cell`}>
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

        {/* What runs this profile without anyone asking, so automation is
            visible from the list instead of one editor at a time. */}
        <td className={CELL_TOP} data-testid={PROFILE_AUTOMATION}>
          <div className="flex flex-wrap gap-1">
            {profile.auto_launch && (
              <span
                className={AUTOMATION_TAG}
                title="The dispatcher may launch this profile"
              >
                auto-launch
              </span>
            )}
            {profile.schedule_cron !== null && (
              <span className={AUTOMATION_TAG}>schedule</span>
            )}
            {!profile.auto_launch && profile.schedule_cron === null && (
              <span className="text-console-muted text-xs">manual</span>
            )}
          </div>
          {/* Below `lg` the schedule rides under its chip; from `lg` up it is
              a column of its own. Either way it is text, never a tooltip
              (`SPEC.md`, "Frontend", "Scheduled profiles"). */}
          {profile.schedule_cron !== null && (
            <ScheduleText
              cron={profile.schedule_cron}
              next={profile.next_scheduled_at}
              className="pt-1 lg:hidden"
            />
          )}
        </td>

        <td className={`${CELL_TOP} hidden lg:table-cell`}>
          {profile.schedule_cron === null ? (
            <span className="text-console-muted text-xs">{PLACEHOLDER}</span>
          ) : (
            <ScheduleText
              cron={profile.schedule_cron}
              next={profile.next_scheduled_at}
            />
          )}
        </td>

        <td
          className={`${CELL_TOP} text-console-muted hidden font-mono text-xs whitespace-nowrap md:table-cell`}
        >
          {String(profile.idle_timeout_secs)}s
        </td>

        <td className={`${CELL_TOP} pr-0`}>
          <div className="flex flex-wrap justify-end gap-1.5">
            <SubmitButton
              type="button"
              variant="ghost"
              disabled={remove.isPending}
              onClick={onEdit}
            >
              Edit
            </SubmitButton>
            {/* A project always keeps one default profile, so the API refuses
                to delete it; the button is disabled before the request and
                described by the note under the name. */}
            <SubmitButton
              type="button"
              variant="danger"
              loading={remove.isPending}
              disabled={profile.is_default || confirming}
              aria-describedby={profile.is_default ? defaultNoteId : undefined}
              onClick={() => {
                setConfirming(true);
              }}
            >
              Delete
            </SubmitButton>
          </div>
        </td>
      </tr>

      {confirming && (
        <tr className={ROW}>
          <td colSpan={COLUMNS.length} className={SPAN_CELL_BARE}>
            <ConfirmPanel
              message={`Delete ${profile.name}? Sessions already running keep their configuration.`}
              confirmLabel={`Delete ${profile.name}`}
              pending={remove.isPending}
              onConfirm={onDelete}
              onCancel={() => {
                setConfirming(false);
              }}
            />
          </td>
        </tr>
      )}

      {remove.isError && (
        <tr className={ROW}>
          <td colSpan={COLUMNS.length} className={SPAN_CELL_BARE}>
            <Alert kind="error">{errorMessage(remove.error)}</Alert>
          </td>
        </tr>
      )}
    </>
  );
}

/** A schedule as the list reads it: the UTC expression and the next run. */
function ScheduleText({
  cron,
  next,
  className,
}: {
  cron: string;
  next: string | null;
  className?: string;
}) {
  return (
    <span
      className={`text-console-muted block font-mono text-xs sm:whitespace-nowrap ${className ?? ""}`}
    >
      <span className="text-console-text">{cron}</span> UTC
      <span className="block">next {formatUtc(next)}</span>
    </span>
  );
}
