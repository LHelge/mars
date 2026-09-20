// `POST /projects/{pid}/sessions` (`SPEC.md`, "Sessions"): the form that
// starts a session from one of the project's agent profiles.
//
// The profile decides what the rest of the form is. A conversational profile
// opens a session somebody keeps talking to, so its first message is optional.
// An ephemeral profile runs one prompt and ends (`SPEC.md`, "User-facing
// features", Agent profiles), so the form becomes "Run with a message" and
// insists on something to run — a message, a task, or both.
//
// Everything else is a default the user may override: the base ref starts at
// the project's integration head, and naming a task that carries a hand-off
// clears it again so the server starts from the hand-off commit instead
// (`SPEC.md`, "Sessions": with no `base_ref`, selection and claim happen
// atomically). Optional fields that are left blank are not sent at all, so the
// server's own defaults — including the title rule — apply.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import { useNavigate } from "react-router";
import {
  Alert,
  FormField,
  SectionHeader,
  SubmitButton,
} from "../../components";
import { useFormSubmit } from "../../hooks/useFormSubmit";
import { listProfiles } from "../../services/profiles";
import { listBranches } from "../../services/projects";
import { queryKeys } from "../../services/queryKeys";
import { createSession } from "../../services/sessions";
import type { Profile, Project, SessionCreateInput } from "../../types";
import { shortSha } from "../../utils/format";
import { BaseRefSelect } from "./BaseRefSelect";
import { useTaskLookup } from "./useTaskLookup";

export interface LaunchSessionFormProps {
  project: Project;
}

/** The length the server truncates a generated title to; over it is allowed. */
const TITLE_SOFT_LIMIT = 80;

const TEXTAREA =
  "border-console-border bg-console-bg text-console-text placeholder:text-console-muted w-full rounded border px-2.5 py-1.5 font-mono text-sm disabled:opacity-50";

function profileLabel(profile: Profile): string {
  const states =
    profile.serves_states.length === 0
      ? "serves no queue"
      : `serves ${profile.serves_states.join(", ")}`;
  return `${profile.name} — ${profile.kind}, ${states}`;
}

export function LaunchSessionForm({ project }: LaunchSessionFormProps) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();

  const ready = project.status === "ready";

  const profiles = useQuery({
    queryKey: queryKeys.projects.profiles(project.id),
    queryFn: () => listProfiles(project.id),
    enabled: ready,
  });

  const branches = useQuery({
    queryKey: queryKeys.projects.branches(project.id),
    queryFn: () => listBranches(project.id),
    enabled: ready,
    retry: false,
  });

  const [profileId, setProfileId] = useState("");
  const [baseRef, setBaseRef] = useState(project.default_branch ?? "");
  const [title, setTitle] = useState("");
  const [message, setMessage] = useState("");
  const [taskRef, setTaskRef] = useState("");

  const rows = profiles.data ?? [];
  // Derived rather than stored, so the project's default profile is selected
  // the moment the list arrives without an effect writing state behind it.
  const selected =
    rows.find((profile) => profile.id === profileId) ??
    rows.find((profile) => profile.is_default) ??
    rows[0];
  const ephemeral = selected?.kind === "ephemeral";

  const lookup = useTaskLookup(project.id, taskRef, ready);
  const task = lookup.status === "found" ? lookup.task : null;
  const handoff = task?.handoff ?? null;

  // A hand-off is the better base than anything the form had preselected, so
  // naming such a task clears the explicit base once — and only once, so the
  // user can still choose one afterwards.
  const clearedFor = useRef<string | null>(null);
  useEffect(() => {
    if (handoff !== null && clearedFor.current !== handoff.id) {
      clearedFor.current = handoff.id;
      setBaseRef("");
    }
  }, [handoff]);

  const trimmedMessage = message.trim();
  const trimmedTitle = title.trim();
  const taskUnresolved = taskRef.trim() !== "" && lookup.status !== "found";
  const needsInput = ephemeral && trimmedMessage === "" && task === null;

  const launch = useFormSubmit(async () => {
    if (selected === undefined) {
      return;
    }
    const input: SessionCreateInput = {
      profile_id: selected.id,
      ...(baseRef.trim() === "" ? {} : { base_ref: baseRef.trim() }),
      ...(trimmedTitle === "" ? {} : { title: trimmedTitle }),
      ...(trimmedMessage === "" ? {} : { message: trimmedMessage }),
      // Always the UUID, even when the user typed a number.
      ...(task === null ? {} : { task_id: task.id }),
    };

    const session = await createSession(project.id, input);
    await queryClient.invalidateQueries({
      queryKey: queryKeys.projects.sessions(project.id),
    });
    void navigate(`/sessions/${session.id}`);
  });

  function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (selected === undefined || taskUnresolved || needsInput) {
      return;
    }
    void launch.submit();
  }

  const disabled = !ready || launch.loading;

  return (
    <form
      onSubmit={onSubmit}
      aria-label={ephemeral ? "Run with a message" : "Launch a session"}
      className="border-console-border bg-console-surface space-y-3 rounded border p-3"
    >
      <SectionHeader
        title={ephemeral ? "Run with a message" : "Launch a session"}
        description={
          ephemeral
            ? "This profile runs one prompt and ends. There is no composer and no follow-up."
            : "A conversational session stays open and keeps running when nobody is watching it."
        }
      />

      {!ready && (
        <Alert kind="info">
          {project.status === "cloning"
            ? "The mirror is still being cloned. Sessions can start once the project is ready."
            : "This project is not ready. Fix the clone before starting a session."}
        </Alert>
      )}

      {launch.error !== null && <Alert kind="error">{launch.error}</Alert>}

      {profiles.isError && (
        <Alert kind="error">Could not load the project&rsquo;s profiles.</Alert>
      )}

      <div className="grid gap-3 sm:grid-cols-2">
        <FormField
          label="Agent profile"
          name="session-profile"
          value={selected?.id ?? ""}
          onChange={setProfileId}
          disabled={disabled}
        >
          <select
            id="session-profile"
            name="session-profile"
            value={selected?.id ?? ""}
            disabled={disabled || rows.length === 0}
            onChange={(event) => {
              setProfileId(event.target.value);
            }}
            className="border-console-border bg-console-bg text-console-text w-full rounded border px-2.5 py-1.5 font-mono text-sm disabled:opacity-50"
          >
            {rows.length === 0 && <option value="">No profiles</option>}
            {rows.map((profile) => (
              <option key={profile.id} value={profile.id}>
                {profileLabel(profile)}
              </option>
            ))}
          </select>
        </FormField>

        <BaseRefSelect
          branches={branches.data ?? []}
          value={baseRef}
          onChange={setBaseRef}
          disabled={disabled}
          defaultLabel={
            handoff === null
              ? `Project default${project.default_branch === null ? "" : ` (${project.default_branch})`}`
              : `Hand-off commit ${shortSha(handoff.commit)}`
          }
          hint={
            handoff !== null && baseRef.trim() !== ""
              ? "Overrides the hand-off base."
              : undefined
          }
        />
      </div>

      <div className="grid gap-3 sm:grid-cols-2">
        <FormField
          label="Title"
          name="session-title"
          value={title}
          onChange={setTitle}
          disabled={disabled}
          hint={
            trimmedTitle.length > TITLE_SOFT_LIMIT
              ? `${String(trimmedTitle.length)} characters — longer than a generated title, which is fine.`
              : "Optional. Defaults to the task's title, or the first line of the message."
          }
        />

        <FormField
          label="Task"
          name="session-task"
          value={taskRef}
          onChange={setTaskRef}
          disabled={disabled}
          hint="Optional. A task number such as #12, or a task id."
          {...(lookup.status === "missing"
            ? { error: "Task not found" }
            : lookup.status === "unreadable"
              ? { error: "Enter a task number such as #12, or a task id." }
              : lookup.status === "error"
                ? { error: lookup.message }
                : {})}
        />
      </div>

      {(lookup.status === "loading" || task !== null) && (
        <p className="text-console-muted text-xs">
          {task === null ? (
            "Looking up the task…"
          ) : (
            <>
              <span className="text-console-text">
                #{task.number} {task.title}
              </span>
              {handoff !== null && (
                <>
                  {" · "}
                  Base: hand-off commit{" "}
                  <span className="font-mono">{shortSha(handoff.commit)}</span>
                  {handoff.source_session_id !== null && (
                    <>
                      {" (from session "}
                      <span className="font-mono">
                        {handoff.source_session_id.slice(0, 8)}
                      </span>
                      )
                    </>
                  )}
                </>
              )}
            </>
          )}
        </p>
      )}

      <div className="flex flex-col gap-1.5">
        <label htmlFor="session-message" className="text-console-muted text-xs">
          {ephemeral ? "Message" : "First message (optional)"}
          {ephemeral && task === null && (
            <span className="text-console-muted"> *</span>
          )}
        </label>
        <textarea
          id="session-message"
          name="session-message"
          rows={4}
          value={message}
          disabled={disabled}
          placeholder={
            ephemeral
              ? "What should this run do?"
              : "What should the agent start with?"
          }
          onChange={(event) => {
            setMessage(event.target.value);
          }}
          className={TEXTAREA}
        />
        {ephemeral && (
          <p className="text-console-muted text-xs">
            An ephemeral session ends after one result, so this is the only
            thing it will be told. A task can stand in for the message.
          </p>
        )}
      </div>

      <div className="flex items-center gap-3">
        <SubmitButton
          loading={launch.loading}
          disabled={
            !ready ||
            selected === undefined ||
            taskUnresolved ||
            needsInput ||
            launch.loading
          }
        >
          {ephemeral ? "Run" : "Launch session"}
        </SubmitButton>
        {needsInput && (
          <span className="text-console-muted text-xs">
            Give this run a message or a task.
          </span>
        )}
      </div>
    </form>
  );
}
