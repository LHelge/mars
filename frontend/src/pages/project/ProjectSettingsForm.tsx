// `PUT /projects/{id}` (`SPEC.md`, "Projects"): the fields a project keeps
// settable after it exists — its name, the default branch sessions start from
// and merges target, how many claims in one state escalate a task
// (`max_attempts`, 1–20), how many revision rounds do (`max_rounds`, 1–50;
// ADR 0046), and the two that bind automation: how many live
// sessions the project may have before an unattended launch is held back, and
// whether unattended launches are paused altogether.
//
// The branch field offers the mirror's integration heads and still accepts a
// name that is not among them, because a branch may exist upstream without
// being in the list yet. Changing the branch while sessions exist is allowed
// by the API, so the form does not refuse it either.
//
// The two automation fields sit in a fieldset of their own, under the same
// heading the profile editor uses for its half of the same rule: what the
// dispatcher may do is one subject, spread over a project and its profiles,
// and it reads as one subject in both places.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import type { FormEvent } from "react";
import { Alert } from "../../components/Alert";
import { FieldShell } from "../../components/FieldShell";
import { FormField } from "../../components/FormField";
import { HelpLink } from "../../components/HelpLink";
import { SectionHeader } from "../../components/SectionHeader";
import { SubmitButton } from "../../components/SubmitButton";
import { FIELD } from "../../components/fieldStyles";
import { useFormSubmit } from "../../hooks/useFormSubmit";
import { updateProject } from "../../services/projects";
import { queryKeys } from "../../services/queryKeys";
import { projectQueries } from "../../services/queryOptions";
import type { Project } from "../../types";
import {
  MAX_ATTEMPTS,
  maxAttemptsError,
  MAX_ROUNDS,
  maxRoundsError,
  MIN_ATTEMPTS,
  MIN_ROUNDS,
  MIN_SESSION_CAP,
  sessionCapError,
  settingsBlocked,
  toProjectUpdate,
  toSettingsState,
} from "./projectSettings";
import type { ProjectSettingsState } from "./projectSettings";

export interface ProjectSettingsFormProps {
  project: Project;
}

export function ProjectSettingsForm({ project }: ProjectSettingsFormProps) {
  const queryClient = useQueryClient();

  const [form, setForm] = useState<ProjectSettingsState>(() =>
    toSettingsState(project),
  );

  // Only a cloned mirror has refs to list; while cloning the field is free
  // text with no suggestions.
  const branches = useQuery({
    ...projectQueries.branches(project.id),
    enabled: project.status === "ready",
  });

  const heads = (branches.data ?? []).filter((ref) => ref.kind === "head");

  // One owner for the save: pending, the refusal and "Saved" all come from
  // here (`CLAUDE.md`, "Frontend conventions", "Submitting a form"). Editing a
  // field resets it, so the banner never describes values that are no longer
  // the ones on screen.
  const save = useFormSubmit(async () => {
    const updated = await updateProject(project.id, toProjectUpdate(form));
    queryClient.setQueryData(queryKeys.projects.detail(project.id), updated);
    await queryClient.invalidateQueries({
      queryKey: queryKeys.projects.list(),
    });
  });

  const attemptsError = maxAttemptsError(form.max_attempts);
  const roundsError = maxRoundsError(form.max_rounds);
  const capError = sessionCapError(form.max_concurrent_sessions);
  const blocked = settingsBlocked(form);

  function patch(next: Partial<ProjectSettingsState>) {
    save.reset();
    setForm((current) => ({ ...current, ...next }));
  }

  function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (blocked) {
      return;
    }
    void save.submit();
  }

  return (
    <form
      onSubmit={onSubmit}
      aria-label="Project settings"
      className="border-console-border bg-console-surface space-y-3 rounded border p-3"
    >
      <SectionHeader
        title="Settings"
        description="Name, the branch sessions start from, the attempt and round limits that escalate a task, and what the dispatcher may launch."
      />

      {save.error !== null && <Alert kind="error">{save.error}</Alert>}
      {save.succeeded && (
        <Alert kind="success" onDismiss={save.reset}>
          Settings saved.
        </Alert>
      )}

      <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
        <FormField
          label="Name"
          name="project-name"
          value={form.name}
          onChange={(next) => {
            patch({ name: next });
          }}
          required
          disabled={save.loading}
        />

        <FieldShell
          label="Default branch"
          name="project-default-branch"
          hint={
            project.default_branch === null
              ? "Still being discovered from the remote."
              : "The Mars branch sessions start from and merges target by default."
          }
          help="branches"
        >
          {(control) => (
            <>
              <input
                {...control}
                list="project-default-branch-options"
                value={form.default_branch}
                onChange={(event) => {
                  patch({ default_branch: event.target.value });
                }}
                disabled={save.loading}
                className={FIELD}
              />
              <datalist id="project-default-branch-options">
                {heads.map((ref) => (
                  <option key={ref.name} value={ref.name} />
                ))}
              </datalist>
            </>
          )}
        </FieldShell>

        <FieldShell
          label="Max attempts"
          name="project-max-attempts"
          hint={`How many times agents may claim a task in one state before a release escalates it to the human state (${String(MIN_ATTEMPTS)}–${String(MAX_ATTEMPTS)}).`}
          help="task-flow"
          error={attemptsError ?? undefined}
        >
          {(control) => (
            <input
              {...control}
              type="number"
              min={MIN_ATTEMPTS}
              max={MAX_ATTEMPTS}
              step={1}
              value={form.max_attempts}
              onChange={(event) => {
                patch({ max_attempts: event.target.value });
              }}
              disabled={save.loading}
              className={FIELD}
            />
          )}
        </FieldShell>

        <FieldShell
          label="Max rounds"
          name="project-max-rounds"
          hint={`How many revision rounds a task may go through before a send-back by an agent or a conflicting automatic merge escalates it to the human state instead (${String(MIN_ROUNDS)}–${String(MAX_ROUNDS)}).`}
          help="task-flow"
          error={roundsError ?? undefined}
        >
          {(control) => (
            <input
              {...control}
              type="number"
              min={MIN_ROUNDS}
              max={MAX_ROUNDS}
              step={1}
              value={form.max_rounds}
              onChange={(event) => {
                patch({ max_rounds: event.target.value });
              }}
              disabled={save.loading}
              className={FIELD}
            />
          )}
        </FieldShell>
      </div>

      <fieldset className="border-console-border rounded border p-3">
        <legend className="text-console-muted px-1 text-xs">
          Unattended launches
        </legend>
        <p className="text-console-muted pb-3 text-xs">
          The cap holds unattended launches back; the pause stops them and
          automatic merges too. You can still launch a session or merge by hand
          while the project is paused or at its cap.{" "}
          <HelpLink topic="automation" />
        </p>

        <div className="grid gap-3 sm:grid-cols-2">
          <FieldShell
            label="Live sessions in this project"
            name="project-max-concurrent-sessions"
            hint="Empty: no cap. Every creating or running session counts, whoever launched it."
            error={capError ?? undefined}
          >
            {(control) => (
              <input
                {...control}
                type="number"
                min={MIN_SESSION_CAP}
                step={1}
                value={form.max_concurrent_sessions}
                onChange={(event) => {
                  patch({ max_concurrent_sessions: event.target.value });
                }}
                placeholder="no cap"
                disabled={save.loading}
                className={FIELD}
              />
            )}
          </FieldShell>

          <label className="text-console-text flex items-start gap-2 self-center text-sm">
            <input
              type="checkbox"
              checked={form.automation_paused}
              onChange={(event) => {
                patch({ automation_paused: event.target.checked });
              }}
              disabled={save.loading}
              className="accent-console-accent mt-1 size-3.5"
            />
            <span>
              Pause automation
              <span className="text-console-muted block text-xs">
                Stops every unattended launch and automatic merge here until it
                is turned off. Sessions already running keep going.
              </span>
            </span>
          </label>
        </div>
      </fieldset>

      <div className="flex items-center gap-2">
        <SubmitButton loading={save.loading} disabled={blocked}>
          Save settings
        </SubmitButton>
      </div>
    </form>
  );
}
