// The drawer's two launch actions (`SPEC.md`, "Frontend", "Task board":
// "open in session", which picks a conversational profile, and "run once",
// which does the same with an ephemeral one; both launch
// `POST /projects/{pid}/sessions` with `task_id`).
//
// The whole point is one click, so everything the server would default is left
// to the server: no `title`, and no `base_ref` unless the user deliberately
// chooses one. The base is therefore shown rather than chosen — the task's
// current hand-off commit when it has one, the project's default branch
// otherwise — and an override is a disclosure the user has to open, because
// starting somewhere else silently would hide the one thing a reviewer needs
// to know (`SPEC.md`, "Frontend", "Hand-off controls": default to the hand-off
// commit and visibly disclose any base override; `ARCHITECTURE.md`,
// "Launching a session for a task": an override confers no approval).
//
// The hand-off can change between opening this form and submitting it. The
// server picks the current one atomically, so the form does not try to keep up
// with it: `useLaunchSession` refetches the task after the answer, whether
// that answer was the new session or the 409 that says somebody else got there
// first.
//
// Buttons, form and submission are one component because the pending state is
// one thing: while a launch is in flight the kind toggles and Cancel are shut,
// so the panel the request belongs to cannot be taken away from under it. The
// drawer can still close on Escape, and a launch whose form is gone lands
// without navigating (`launch/useLaunchSession.ts`).

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import type { FormEvent } from "react";
import { Link, useNavigate } from "react-router";

import { Alert } from "../components/Alert";
import { FieldShell } from "../components/FieldShell";
import { FIELD } from "../components/fieldStyles";
import { SubmitButton } from "../components/SubmitButton";
import { useFormSubmit } from "../hooks/useFormSubmit";
import {
  BaseRefSelect,
  HandoffBaseNote,
  ProfileSelect,
  baseRefDefaultLabel,
  useLaunchSession,
} from "../launch";
import { AgentCredentialNotice } from "../secrets/AgentCredentialNotice";
import { useAgentCredential } from "../secrets/useAgentCredential";
import { projectQueries } from "../services/queryOptions";
import type {
  Profile,
  ProfileKind,
  SessionCreateInput,
  TaskDetail,
} from "../types";
import { defaultProfile, launchDisabledReason } from "./launchRules";
import { useTaskStore } from "./taskStore";

export interface LaunchForTaskProps {
  projectId: string;
  task: TaskDetail;
}

/** What each button says, and what its form is called once it is open. */
const ACTION: Record<ProfileKind, string> = {
  conversational: "Open in session",
  ephemeral: "Run once",
};

/** How an entry of the profile select reads in this form. */
function profileLabel(profile: Profile, stateName: string): string {
  return profile.serves_states.includes(stateName)
    ? `${profile.name} — serves ${stateName}`
    : profile.name;
}

export function LaunchForTask({ projectId, task }: LaunchForTaskProps) {
  // Which form is open, if either. Switching from one button to the other
  // reuses the same form with a different profile kind, and starts it over:
  // the profile, the message and the base a user chose for a conversation do
  // not belong to a one-off run.
  const [kind, setKind] = useState<ProfileKind | null>(null);
  const [profileId, setProfileId] = useState("");
  const [baseRef, setBaseRef] = useState("");
  const [message, setMessage] = useState("");

  const navigate = useNavigate();

  const stateKind = useTaskStore(
    (store) => store.states.find((state) => state.name === task.state)?.kind,
  );

  const project = useQuery(projectQueries.detail(projectId));

  // Only once a form is open: an unopened launch control needs neither the
  // project's profiles nor its refs.
  const profiles = useQuery({
    ...projectQueries.profiles(projectId),
    enabled: kind !== null,
  });
  const branches = useQuery({
    ...projectQueries.branches(projectId),
    enabled: kind !== null,
  });

  const reason = launchDisabledReason(task, stateKind, project.data?.status);
  const open = reason === null && kind !== null;

  const rows =
    kind === null
      ? []
      : (profiles.data ?? []).filter((profile) => profile.kind === kind);
  // Derived, not stored: the default is right the moment the list arrives,
  // without an effect writing state behind the render.
  const selected =
    kind === null
      ? undefined
      : (rows.find((profile) => profile.id === profileId) ??
        defaultProfile(profiles.data ?? [], kind, task.state));

  const override = baseRef.trim();
  const handoff = task.handoff;

  // The same answer the profile editor and the project's launch form show, for
  // the profile selected here (`SPEC.md`, "Frontend", Agent credentials). It
  // warns and renames the button; it never blocks the launch (ADR 0036).
  const { credential } = useAgentCredential(projectId, selected?.backend ?? "");
  const noCredential = credential === null;

  const launchSession = useLaunchSession(projectId, task.number);

  const launch = useFormSubmit(async () => {
    if (selected === undefined) {
      return;
    }
    await launchSession({
      profile_id: selected.id,
      task_id: task.id,
      ...(override === "" ? {} : { base_ref: override }),
      ...(message.trim() === "" ? {} : { message: message.trim() }),
    } satisfies SessionCreateInput);
  });

  /** Open this kind's form, or shut the one that is open, and start it over. */
  function toggle(candidate: ProfileKind) {
    setKind(kind === candidate ? null : candidate);
    setProfileId("");
    setBaseRef("");
    setMessage("");
    launch.reset();
  }

  function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (selected === undefined) {
      return;
    }
    void launch.submit();
  }

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center gap-2">
        {(["conversational", "ephemeral"] as const).map((candidate) => (
          <span key={candidate} title={reason ?? undefined}>
            <button
              type="button"
              // Shut while a launch is in flight: the request belongs to the
              // form below, and switching kinds under it would leave the
              // answer describing a form that is no longer there.
              disabled={reason !== null || launch.loading}
              title={reason ?? undefined}
              aria-expanded={kind === candidate}
              onClick={() => {
                toggle(candidate);
              }}
              className="border-console-border text-console-muted hover:text-console-text hover:bg-console-raised rounded border bg-transparent px-3 py-1.5 text-sm disabled:opacity-60 disabled:hover:bg-transparent"
            >
              {ACTION[candidate]}
            </button>
          </span>
        ))}
      </div>

      {open && kind !== null && (
        <form
          onSubmit={onSubmit}
          aria-label={ACTION[kind]}
          className="border-console-border bg-console-bg space-y-3 rounded border p-3"
        >
          {launch.error !== null && <Alert kind="error">{launch.error}</Alert>}

          {profiles.isError && (
            <Alert kind="error">
              Could not load the project&rsquo;s profiles.
            </Alert>
          )}

          {rows.length === 0 && !profiles.isLoading && !profiles.isError ? (
            <p className="text-console-muted text-sm">
              No {kind} profile in this project —{" "}
              <Link
                to={`/projects/${projectId}?tab=profiles`}
                className="text-console-accent underline"
              >
                add one
              </Link>
            </p>
          ) : (
            <ProfileSelect
              name="launch-profile"
              profiles={rows}
              selected={selected}
              onChange={setProfileId}
              optionLabel={(profile) => profileLabel(profile, task.state)}
              disabled={launch.loading}
            />
          )}

          <HandoffBaseNote
            handoff={handoff}
            defaultBranch={project.data?.default_branch ?? null}
            override={override}
          />

          {/* The override is a disclosure, not a field of the form: choosing a
              base is the exception, and the same control the project page
              offers is behind it. */}
          <details>
            <summary className="text-console-muted cursor-pointer text-xs">
              Override base ref
            </summary>
            <div className="mt-1.5">
              <BaseRefSelect
                name="launch-base-ref"
                branches={branches.data ?? []}
                value={baseRef}
                onChange={setBaseRef}
                disabled={launch.loading}
                defaultLabel={baseRefDefaultLabel(
                  handoff,
                  project.data?.default_branch ?? null,
                )}
              />
            </div>
          </details>

          <FieldShell label="First message (optional)" name="launch-message">
            {(control) => (
              <textarea
                {...control}
                rows={3}
                value={message}
                disabled={launch.loading}
                placeholder="What should the agent start with?"
                onChange={(event) => {
                  setMessage(event.target.value);
                }}
                className={FIELD}
              />
            )}
          </FieldShell>

          {selected !== undefined && (
            <AgentCredentialNotice
              projectId={projectId}
              backend={selected.backend}
            />
          )}

          <div className="flex items-center gap-2">
            {noCredential && (
              <SubmitButton
                type="button"
                onClick={() => {
                  void navigate("/secrets");
                }}
              >
                Add credential
              </SubmitButton>
            )}
            <SubmitButton
              variant={noCredential ? "ghost" : "primary"}
              loading={launch.loading}
              disabled={selected === undefined}
            >
              {noCredential ? "Launch anyway" : ACTION[kind]}
            </SubmitButton>
            <SubmitButton
              type="button"
              variant="ghost"
              disabled={launch.loading}
              onClick={() => {
                toggle(kind);
              }}
            >
              Cancel
            </SubmitButton>
          </div>
        </form>
      )}
    </div>
  );
}
