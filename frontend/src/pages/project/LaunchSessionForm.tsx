// `POST /projects/{pid}/sessions` (`SPEC.md`, "Sessions"): the form that
// starts a session from one of the project's agent profiles.
//
// The profile decides what the rest of the form is. A conversational profile
// opens a session somebody keeps talking to, so its first message is optional.
// An ephemeral profile runs one prompt and ends (`SPEC.md`, "User-facing
// features", Agent profiles), so the form becomes "Run with a message" and
// insists on something to run — a message, a task, or both.
//
// Everything else is left to the server. The base ref starts empty, which is
// how "whatever the server would choose" is asked for — the task's hand-off
// commit when the named task has one, otherwise the project's default branch,
// decided when the request arrives and not when this form was rendered
// (`SPEC.md`, "Sessions": with no `base_ref`, selection and claim happen
// atomically). Optional fields that are left blank are not sent at all, so the
// server's own defaults — including the title rule — apply.
//
// The launch itself — create, the two caches it invalidates and the navigation
// — is `useLaunchSession`, shared with the task drawer's form.

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import type { FormEvent } from "react";
import { useNavigate } from "react-router";
import { Alert } from "../../components/Alert";
import { FieldShell } from "../../components/FieldShell";
import { FormField } from "../../components/FormField";
import { SectionHeader } from "../../components/SectionHeader";
import { Icon } from "../../components/icons";
import { SubmitButton } from "../../components/SubmitButton";
import { FIELD } from "../../components/fieldStyles";
import { useFormSubmit } from "../../hooks/useFormSubmit";
import {
  BaseRefSelect,
  HandoffBaseNote,
  ProfileSelect,
  baseRefDefaultLabel,
  useLaunchSession,
} from "../../launch";
import { AgentCredentialNotice } from "../../secrets/AgentCredentialNotice";
import { useAgentCredential } from "../../secrets/useAgentCredential";
import { projectQueries } from "../../services/queryOptions";
import type { Profile, Project, SessionCreateInput } from "../../types";
import { useTaskLookup } from "./useTaskLookup";
import type { TaskLookup } from "./useTaskLookup";

export interface LaunchSessionFormProps {
  project: Project;
}

/** The length the server truncates a generated title to; over it is allowed. */
const TITLE_SOFT_LIMIT = 80;

/** What the Task field says when the reference does not resolve. */
function taskFieldError(lookup: TaskLookup): string | undefined {
  switch (lookup.status) {
    case "missing":
      return "Task not found";
    case "unreadable":
      return "Enter a task number such as #12, or a task id.";
    case "error":
      return lookup.message;
    default:
      return undefined;
  }
}

function profileLabel(profile: Profile): string {
  const states =
    profile.serves_states.length === 0
      ? "serves no queue"
      : `serves ${profile.serves_states.join(", ")}`;
  return `${profile.name} — ${profile.kind}, ${states}`;
}

export function LaunchSessionForm({ project }: LaunchSessionFormProps) {
  const navigate = useNavigate();

  const ready = project.status === "ready";

  const profiles = useQuery({
    ...projectQueries.profiles(project.id),
    enabled: ready,
  });

  const branches = useQuery({
    ...projectQueries.branches(project.id),
    enabled: ready,
  });

  const [profileId, setProfileId] = useState("");
  const [baseRef, setBaseRef] = useState("");
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

  // Which credential this launch would use, for the profile that is selected
  // right now (`SPEC.md`, "Frontend", Agent credentials). A missing one warns
  // and renames the button; it never blocks, because the stub image and an
  // image with its own authentication need none (ADR 0036).
  const { credential } = useAgentCredential(
    project.id,
    selected?.backend ?? "",
  );
  const noCredential = credential === null;

  const lookup = useTaskLookup(project.id, taskRef, ready);
  const task = lookup.status === "found" ? lookup.task : null;
  const handoff = task?.handoff ?? null;

  const trimmedMessage = message.trim();
  const trimmedTitle = title.trim();
  const trimmedBase = baseRef.trim();
  const taskUnresolved = taskRef.trim() !== "" && lookup.status !== "found";
  const needsInput = ephemeral && trimmedMessage === "" && task === null;

  const launchSession = useLaunchSession(project.id, task?.number ?? null);

  const launch = useFormSubmit(async () => {
    if (selected === undefined) {
      return;
    }
    await launchSession({
      profile_id: selected.id,
      ...(trimmedBase === "" ? {} : { base_ref: trimmedBase }),
      ...(trimmedTitle === "" ? {} : { title: trimmedTitle }),
      ...(trimmedMessage === "" ? {} : { message: trimmedMessage }),
      // Always the UUID, even when the user typed a number.
      ...(task === null ? {} : { task_id: task.id }),
    } satisfies SessionCreateInput);
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
        <ProfileSelect
          name="session-profile"
          profiles={rows}
          selected={selected}
          onChange={setProfileId}
          optionLabel={profileLabel}
          emptyLabel="No profiles"
          disabled={disabled}
        />

        <BaseRefSelect
          branches={branches.data ?? []}
          value={baseRef}
          onChange={setBaseRef}
          disabled={disabled}
          defaultLabel={baseRefDefaultLabel(
            handoff,
            project.default_branch ?? null,
          )}
          hint={
            handoff !== null && trimmedBase !== ""
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
          error={taskFieldError(lookup)}
        />
      </div>

      {lookup.status === "loading" && (
        <p className="text-console-muted text-xs">Looking up the task…</p>
      )}

      {task !== null && (
        <>
          <p className="text-console-muted text-xs">
            <span className="text-console-text">
              #{task.number} {task.title}
            </span>
          </p>
          {/* The same base disclosure the drawer's launch shows, because it is
              the same launch (`SPEC.md`, "Frontend", "Hand-off controls"). */}
          <HandoffBaseNote
            handoff={handoff}
            defaultBranch={project.default_branch ?? null}
            override={trimmedBase}
          />
        </>
      )}

      <FieldShell
        label={ephemeral ? "Message" : "First message (optional)"}
        name="session-message"
        required={ephemeral && task === null}
        hint={
          ephemeral
            ? "An ephemeral session ends after one result, so this is the only thing it will be told. A task can stand in for the message."
            : undefined
        }
        help="instructions"
      >
        {(control) => (
          <textarea
            {...control}
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
            className={FIELD}
          />
        )}
      </FieldShell>

      {selected !== undefined && (
        <AgentCredentialNotice
          projectId={project.id}
          backend={selected.backend}
        />
      )}

      <div className="flex items-center gap-3">
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
          icon={Icon.launch}
          disabled={
            !ready ||
            selected === undefined ||
            taskUnresolved ||
            needsInput ||
            launch.loading
          }
        >
          {noCredential
            ? "Launch anyway"
            : ephemeral
              ? "Run"
              : "Launch session"}
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
