// `PUT /projects/{id}` (`SPEC.md`, "Projects"): the three fields a project
// keeps settable after it exists — its name, the integration branch sessions
// are based on and branched from, and how many claims in one state escalate a
// task (`max_attempts`, 1–20).
//
// The branch field offers the mirror's integration heads and still accepts a
// name that is not among them, because a branch may exist upstream without
// being in the list yet. Changing the branch while sessions exist is allowed
// by the API, so the form does not refuse it either.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import type { FormEvent } from "react";
import {
  Alert,
  FieldShell,
  FIELD,
  FormField,
  SectionHeader,
  SubmitButton,
} from "../../components";
import { listBranches, updateProject } from "../../services/projects";
import { queryKeys } from "../../services/queryKeys";
import type { Project } from "../../types";
import { errorMessage, logUnexpected } from "../../services/errorMessage";

export interface ProjectSettingsFormProps {
  project: Project;
}

const MIN_ATTEMPTS = 1;
const MAX_ATTEMPTS = 20;

export function ProjectSettingsForm({ project }: ProjectSettingsFormProps) {
  const queryClient = useQueryClient();

  const [name, setName] = useState(project.name);
  const [branch, setBranch] = useState(project.default_branch ?? "");
  const [attempts, setAttempts] = useState(String(project.max_attempts));
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  // Only a cloned mirror has refs to list; while cloning the field is free
  // text with no suggestions.
  const branches = useQuery({
    queryKey: queryKeys.projects.branches(project.id),
    queryFn: () => listBranches(project.id),
    enabled: project.status === "ready",
  });

  const heads = (branches.data ?? []).filter((ref) => ref.kind === "head");

  const save = useMutation({
    mutationFn: () =>
      updateProject(project.id, {
        name,
        ...(branch === "" ? {} : { default_branch: branch }),
        max_attempts: Number(attempts),
      }),
    onSuccess: (updated: Project) => {
      queryClient.setQueryData(queryKeys.projects.detail(project.id), updated);
      void queryClient.invalidateQueries({
        queryKey: queryKeys.projects.list(),
      });
      setError(null);
      setSaved(true);
    },
    onError: (caught: unknown) => {
      setSaved(false);
      logUnexpected(caught);
      setError(errorMessage(caught));
    },
  });

  const attemptsNumber = Number(attempts);
  const attemptsInvalid =
    !Number.isInteger(attemptsNumber) ||
    attemptsNumber < MIN_ATTEMPTS ||
    attemptsNumber > MAX_ATTEMPTS;

  function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (attemptsInvalid || name.trim() === "") {
      return;
    }
    save.mutate();
  }

  return (
    <form
      onSubmit={onSubmit}
      aria-label="Project settings"
      className="border-console-border bg-console-surface space-y-3 rounded border p-3"
    >
      <SectionHeader
        title="Settings"
        description="Name, the integration branch sessions are based on, and the attempt limit that escalates a task."
      />

      {error !== null && <Alert kind="error">{error}</Alert>}
      {saved && error === null && (
        <Alert
          kind="success"
          onDismiss={() => {
            setSaved(false);
          }}
        >
          Settings saved.
        </Alert>
      )}

      <div className="grid gap-3 sm:grid-cols-3">
        <FormField
          label="Name"
          name="project-name"
          value={name}
          onChange={(next) => {
            setName(next);
            setSaved(false);
          }}
          required
          disabled={save.isPending}
        />

        <FieldShell
          label="Default branch"
          name="project-default-branch"
          hint={
            project.default_branch === null
              ? "Still being discovered from the remote."
              : "An integration head, or any branch name."
          }
        >
          {(control) => (
            <>
              <input
                {...control}
                list="project-default-branch-options"
                value={branch}
                onChange={(event) => {
                  setBranch(event.target.value);
                  setSaved(false);
                }}
                disabled={save.isPending}
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
          hint={`${String(MIN_ATTEMPTS)}–${String(MAX_ATTEMPTS)}`}
          error={
            attemptsInvalid
              ? `Between ${String(MIN_ATTEMPTS)} and ${String(MAX_ATTEMPTS)}.`
              : undefined
          }
        >
          {(control) => (
            <input
              {...control}
              type="number"
              min={MIN_ATTEMPTS}
              max={MAX_ATTEMPTS}
              step={1}
              value={attempts}
              onChange={(event) => {
                setAttempts(event.target.value);
                setSaved(false);
              }}
              disabled={save.isPending}
              className={FIELD}
            />
          )}
        </FieldShell>
      </div>

      <div className="flex items-center gap-2">
        <SubmitButton
          loading={save.isPending}
          disabled={attemptsInvalid || name.trim() === ""}
        >
          Save settings
        </SubmitButton>
      </div>
    </form>
  );
}
