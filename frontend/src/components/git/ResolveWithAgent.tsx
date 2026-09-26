// "Resolve with an agent" under a merge that answered 422 with `conflicts`
// (`SPEC.md`, "Frontend", the git panel's conflict answer; "User-facing
// features", Git operations).
//
// Everything the resolution needs already works: a session's clone has the
// project repository as `origin` and can fetch the source with an explicit
// refspec. What this adds is the launch: a conversational session started from
// the merge's **target**, whose first message names the source ref, its commit
// and the conflicting paths the way the `resolver` role template expects them
// (`SPEC.md`, "Role profile templates" → `resolver`). The message is generated
// and shown for editing, never sent unseen.
//
// The profile list is the cached read the launch form uses; the profile named
// `resolver` is preselected when the project has one, otherwise the default.
// Nothing here merges: the resolver commits on its own branch and a person
// merges that branch like any other.

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { Link } from "react-router";

import { ProfileSelect } from "../../launch/ProfileSelect";
import { useLaunchSession } from "../../launch/useLaunchSession";
import { AgentCredentialNotice } from "../../secrets/AgentCredentialNotice";
import { projectQueries } from "../../services/queryOptions";
import { profilesPath } from "../../tasks/taskLink";
import type { Profile } from "../../types";
import { RESOLVE_WITH_AGENT } from "../../utils/testIds";
import { useFormSubmit } from "../../hooks/useFormSubmit";
import { Alert } from "../Alert";
import { FieldShell } from "../FieldShell";
import { FIELD } from "../fieldStyles";
import { Icon } from "../icons";
import { SubmitButton } from "../SubmitButton";
import { ReadOnlyField } from "./ReadOnlyField";
import {
  conversationalProfiles,
  resolveMessage,
  selectedResolver,
} from "./resolveMessage";
import type { ResolveMerge } from "./resolveMessage";

export interface ResolveWithAgentProps {
  projectId: string;
  /** The merge that conflicted, as the form that ran it knows it. */
  merge: ResolveMerge;
  /** Disabled while another git form of the panel is in flight. */
  disabled: boolean;
}

function profileLabel(profile: Profile): string {
  return profile.is_default ? `${profile.name} (default)` : profile.name;
}

export function ResolveWithAgent({
  projectId,
  merge,
  disabled,
}: ResolveWithAgentProps) {
  const [open, setOpen] = useState(false);

  return (
    <div className="flex flex-col gap-3">
      <div>
        <SubmitButton
          type="button"
          variant="ghost"
          icon={Icon.resolve}
          aria-expanded={open}
          disabled={disabled}
          onClick={() => {
            setOpen((current) => !current);
          }}
        >
          Resolve with an agent
        </SubmitButton>
      </div>
      {open && (
        <ResolveForm
          projectId={projectId}
          merge={merge}
          onCancel={() => {
            setOpen(false);
          }}
        />
      )}
    </div>
  );
}

function ResolveForm({
  projectId,
  merge,
  onCancel,
}: {
  projectId: string;
  merge: ResolveMerge;
  onCancel: () => void;
}) {
  const profiles = useQuery(projectQueries.profiles(projectId));
  const offered = conversationalProfiles(profiles.data ?? []);
  const [profileId, setProfileId] = useState("");
  // Derived, like the launch form's: the preselection holds the moment the
  // list arrives, without an effect writing state behind the render.
  const selected = selectedResolver(offered, profileId);

  const generated = resolveMessage(merge);
  const [edited, setEdited] = useState<string | null>(null);
  const message = edited ?? generated;
  const trimmed = message.trim();

  const launchSession = useLaunchSession(projectId, null);

  const launch = useFormSubmit(async () => {
    if (selected === undefined || trimmed === "") {
      return;
    }
    // The one write that leaves this screen: `useLaunchSession` starts the
    // session-list invalidation without awaiting it and navigates to the new
    // session, which is what proves the launch; this panel is on its way out
    // (`CLAUDE.md`, "Submitting a form", the launch forms' exception).
    await launchSession({
      profile_id: selected.id,
      base_ref: merge.target,
      message: trimmed,
    });
  });

  const busy = launch.loading;
  const noProfile =
    profiles.isSuccess && offered.length === 0 ? (
      <p className="text-console-muted text-sm">
        This project has no conversational profile to launch.{" "}
        <Link
          to={profilesPath(projectId)}
          className="text-console-accent underline"
        >
          Add one on the Profiles tab
        </Link>
        , for example from the <span className="font-mono">resolver</span>{" "}
        template.
      </p>
    ) : null;

  return (
    <form
      data-testid={RESOLVE_WITH_AGENT}
      aria-label="Resolve with an agent"
      className="border-console-border bg-console-bg flex flex-col gap-3 rounded border p-3"
      onSubmit={(event) => {
        event.preventDefault();
        void launch.submit();
      }}
    >
      <p className="text-console-muted max-w-prose text-xs">
        Starts a conversational session from{" "}
        <span className="font-mono">{merge.target}</span> that merges the source
        into its own branch and resolves the conflicts. Nothing is merged into{" "}
        <span className="font-mono">{merge.target}</span>: you merge the
        session&apos;s branch once it says it is ready.
      </p>

      {profiles.isError && (
        <Alert kind="error">Could not load the project&rsquo;s profiles.</Alert>
      )}

      {noProfile ?? (
        <>
          <div className="grid gap-3 sm:grid-cols-2">
            <ProfileSelect
              name="resolve-profile"
              profiles={offered}
              selected={selected}
              onChange={setProfileId}
              optionLabel={profileLabel}
              emptyLabel={profiles.isPending ? "Loading profiles…" : undefined}
              disabled={busy}
            />
            <ReadOnlyField label="Base ref" value={merge.target} />
          </div>

          <FieldShell
            label="First message"
            name="resolve-message"
            hint="Generated from the conflict. Edit it before launching if the agent needs more to go on."
            help="instructions"
          >
            {(control) => (
              <textarea
                {...control}
                rows={Math.min(14, message.split("\n").length + 1)}
                value={message}
                disabled={busy}
                onChange={(event) => {
                  setEdited(event.target.value);
                }}
                className={`${FIELD} font-mono text-xs`}
              />
            )}
          </FieldShell>

          {selected !== undefined && (
            <AgentCredentialNotice
              projectId={projectId}
              backend={selected.backend}
            />
          )}

          {launch.error !== null && (
            <Alert kind="error" onDismiss={launch.reset}>
              {launch.error}
            </Alert>
          )}

          <div className="flex flex-wrap items-center gap-3">
            <SubmitButton
              loading={busy}
              icon={Icon.launch}
              disabled={busy || selected === undefined || trimmed === ""}
            >
              Launch resolver session
            </SubmitButton>
            <SubmitButton
              type="button"
              variant="ghost"
              disabled={busy}
              onClick={onCancel}
            >
              Cancel
            </SubmitButton>
            {edited !== null && edited !== generated && (
              <SubmitButton
                type="button"
                variant="ghost"
                disabled={busy}
                icon={Icon.retry}
                onClick={() => {
                  setEdited(null);
                }}
              >
                Restore generated message
              </SubmitButton>
            )}
          </div>
        </>
      )}
    </form>
  );
}
