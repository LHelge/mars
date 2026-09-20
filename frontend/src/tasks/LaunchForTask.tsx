// The drawer's two launch actions (`SPEC.md`, "Frontend", "Task board":
// "open in session", which picks a conversational profile, and "run once",
// which does the same with an ephemeral one; both launch
// `POST /projects/{pid}/sessions` with `task_id`).
//
// The whole point is one click, so everything the server would default is left
// to the server: no `title`, and no `base_ref` unless the user deliberately
// types one. The base is therefore shown rather than chosen — the task's
// current hand-off commit when it has one, the project's default branch
// otherwise — and an override is a disclosure the user has to open, because
// starting somewhere else silently would hide the one thing a reviewer needs
// to know (`SPEC.md`, "Frontend", "Hand-off controls": default to the hand-off
// commit and visibly disclose any base override; `ARCHITECTURE.md`,
// "Launching a session for a task": an override confers no approval).
//
// The hand-off can change between opening this form and submitting it. The
// server picks the current one atomically, so the form does not try to keep up
// with it: it refetches the task after the answer, whether that answer was the
// new session or the 409 that says somebody else got there first.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import type { FormEvent } from "react";
import { Link, useNavigate } from "react-router";

import { Alert } from "../components/Alert";
import { SubmitButton } from "../components/SubmitButton";
import { useFormSubmit } from "../hooks/useFormSubmit";
import { ApiError } from "../services/apiClient";
import { listProfiles } from "../services/profiles";
import { getProject } from "../services/projects";
import { queryKeys } from "../services/queryKeys";
import { createSession } from "../services/sessions";
import type { ProfileKind, SessionCreateInput, TaskDetail } from "../types";
import { taskKeys } from "./queryKeys";
import {
  defaultProfile,
  launchDisabledReason,
  shortCommit,
} from "./launchRules";
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

const CONTROL =
  "border-console-border bg-console-bg text-console-text placeholder:text-console-muted w-full rounded border px-2.5 py-1.5 font-mono text-sm disabled:opacity-50";

export function LaunchForTask({ projectId, task }: LaunchForTaskProps) {
  // Which form is open, if either. Switching from one button to the other
  // reuses the same form with a different profile kind.
  const [kind, setKind] = useState<ProfileKind | null>(null);

  const stateKind = useTaskStore(
    (store) => store.states.find((state) => state.name === task.state)?.kind,
  );

  const project = useQuery({
    queryKey: queryKeys.projects.detail(projectId),
    queryFn: () => getProject(projectId),
  });

  const reason = launchDisabledReason(task, stateKind, project.data?.status);

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center gap-2">
        {(["conversational", "ephemeral"] as const).map((candidate) => (
          <span key={candidate} title={reason ?? undefined}>
            <button
              type="button"
              disabled={reason !== null}
              title={reason ?? undefined}
              aria-expanded={kind === candidate}
              onClick={() => {
                setKind(kind === candidate ? null : candidate);
              }}
              className="border-console-border text-console-muted hover:text-console-text hover:bg-console-raised rounded border bg-transparent px-3 py-1.5 text-sm disabled:opacity-60 disabled:hover:bg-transparent"
            >
              {ACTION[candidate]}
            </button>
          </span>
        ))}
      </div>

      {reason === null && kind !== null && (
        <LaunchFormPanel
          // A fresh form per kind: the profile, the message and the override
          // the user chose for a conversation do not belong to a one-off run.
          key={kind}
          projectId={projectId}
          task={task}
          kind={kind}
          onClose={() => {
            setKind(null);
          }}
        />
      )}
    </div>
  );
}

interface LaunchFormPanelProps {
  projectId: string;
  task: TaskDetail;
  kind: ProfileKind;
  onClose: () => void;
}

function LaunchFormPanel({
  projectId,
  task,
  kind,
  onClose,
}: LaunchFormPanelProps) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();

  const project = useQuery({
    queryKey: queryKeys.projects.detail(projectId),
    queryFn: () => getProject(projectId),
  });

  const profiles = useQuery({
    queryKey: queryKeys.projects.profiles(projectId),
    queryFn: () => listProfiles(projectId),
  });

  const [profileId, setProfileId] = useState("");
  const [baseRef, setBaseRef] = useState("");
  const [message, setMessage] = useState("");

  const rows = (profiles.data ?? []).filter((profile) => profile.kind === kind);
  // Derived, not stored: the default is right the moment the list arrives,
  // without an effect writing state behind the render.
  const selected =
    rows.find((profile) => profile.id === profileId) ??
    defaultProfile(profiles.data ?? [], kind, task.state);

  const override = baseRef.trim();
  const handoff = task.handoff;

  const launch = useFormSubmit(async () => {
    if (selected === undefined) {
      return;
    }
    const input: SessionCreateInput = {
      profile_id: selected.id,
      task_id: task.id,
      ...(override === "" ? {} : { base_ref: override }),
      ...(message.trim() === "" ? {} : { message: message.trim() }),
    };

    try {
      const session = await createSession(projectId, input);
      useTaskStore.getState().invalidate();
      await queryClient.invalidateQueries({
        queryKey: taskKeys.detail(projectId, task.number),
      });
      void navigate(`/sessions/${session.id}`);
    } catch (caught) {
      // 409 is somebody else holding the task now, or a hand-off that moved.
      // Whatever the drawer is showing about it is already out of date.
      if (caught instanceof ApiError && caught.status === 409) {
        void queryClient.invalidateQueries({
          queryKey: taskKeys.detail(projectId, task.number),
        });
      }
      throw caught;
    }
  });

  function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (selected === undefined) {
      return;
    }
    void launch.submit();
  }

  return (
    <form
      onSubmit={onSubmit}
      aria-label={ACTION[kind]}
      className="border-console-border bg-console-bg space-y-3 rounded border p-3"
    >
      {launch.error !== null && <Alert kind="error">{launch.error}</Alert>}

      {profiles.isError && (
        <Alert kind="error">Could not load the project&rsquo;s profiles.</Alert>
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
        <div className="flex flex-col gap-1.5">
          <label
            htmlFor="launch-profile"
            className="text-console-muted text-xs"
          >
            Agent profile
          </label>
          <select
            id="launch-profile"
            name="launch-profile"
            value={selected?.id ?? ""}
            disabled={launch.loading || rows.length === 0}
            onChange={(event) => {
              setProfileId(event.target.value);
            }}
            className={CONTROL}
          >
            {rows.map((profile) => (
              <option key={profile.id} value={profile.id}>
                {profile.serves_states.includes(task.state)
                  ? `${profile.name} — serves ${task.state}`
                  : profile.name}
              </option>
            ))}
          </select>
        </div>
      )}

      <div className="flex flex-col gap-1.5">
        <p className="text-console-text text-sm">
          {handoff === null ? (
            <>Base: {project.data?.default_branch ?? "the project default"}</>
          ) : (
            <>
              Base: hand-off{" "}
              <span className="font-mono" title={handoff.commit}>
                {shortCommit(handoff.commit)}
              </span>{" "}
              from {handoff.source_branch} ({handoff.review_status})
            </>
          )}
        </p>

        <details>
          <summary className="text-console-muted cursor-pointer text-xs">
            Override base ref
          </summary>
          <input
            id="launch-base-ref"
            name="launch-base-ref"
            value={baseRef}
            disabled={launch.loading}
            placeholder="A branch, tag or commit id"
            aria-label="Override base ref"
            onChange={(event) => {
              setBaseRef(event.target.value);
            }}
            className={`${CONTROL} mt-1.5`}
          />
        </details>

        {override !== "" && (
          <Alert kind="warning">
            Base overridden: the session will not start from the hand-off commit
            and this grants no review approval
          </Alert>
        )}
      </div>

      <div className="flex flex-col gap-1.5">
        <label htmlFor="launch-message" className="text-console-muted text-xs">
          First message (optional)
        </label>
        <textarea
          id="launch-message"
          name="launch-message"
          rows={3}
          value={message}
          disabled={launch.loading}
          placeholder="What should the agent start with?"
          onChange={(event) => {
            setMessage(event.target.value);
          }}
          className={CONTROL}
        />
      </div>

      <div className="flex items-center gap-2">
        <SubmitButton
          loading={launch.loading}
          disabled={selected === undefined}
        >
          {ACTION[kind]}
        </SubmitButton>
        <SubmitButton
          type="button"
          variant="ghost"
          loading={false}
          onClick={onClose}
        >
          Cancel
        </SubmitButton>
      </div>
    </form>
  );
}
