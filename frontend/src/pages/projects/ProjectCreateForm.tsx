// `POST /projects` (`SPEC.md`, "Projects"): the panel that starts a clone. The
// request returns immediately with `status: cloning`, so the form's job ends
// at 201 — the list polls the rest of the way and this navigates to the new
// project.
//
// `credential` is the private-repository token. It is sent once, stored by the
// orchestrator as the project-scoped `GIT_CREDENTIAL` secret and never comes
// back (`CLAUDE.md`, rule 3), so it lives in component state only until the
// request settles and is cleared whichever way that goes.

import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import type { FormEvent } from "react";
import { useNavigate } from "react-router";
import { Alert, FormField, SubmitButton } from "../../components";
import { useFormSubmit } from "../../hooks";
import { ApiError } from "../../services/apiClient";
import { createProject } from "../../services/projects";
import { queryKeys } from "../../services/queryKeys";
import type { Project } from "../../types";

/** `docs/data-model.md`, `projects.name`: 1–100 characters, unique. */
const NAME_MAX = 100;

interface FieldErrors {
  name?: string;
  remoteUrl?: string;
}

/**
 * The client-side half of the contract, checked before the request so the
 * obvious mistakes never cost a round trip. Uniqueness is the server's answer
 * and arrives as a 409.
 */
function validateProjectForm(
  name: string,
  remoteUrl: string,
): FieldErrors {
  const errors: FieldErrors = {};

  const trimmedName = name.trim();
  if (trimmedName === "") {
    errors.name = "A name is required.";
  } else if (trimmedName.length > NAME_MAX) {
    errors.name = `At most ${NAME_MAX} characters.`;
  }

  const url = remoteUrl.trim();
  if (url === "") {
    errors.remoteUrl = "A remote URL is required.";
  } else if (!url.startsWith("https://")) {
    errors.remoteUrl = "The remote URL must start with https://.";
  } else if (url.slice("https://".length).split("/")[0].includes("@")) {
    errors.remoteUrl = "Put the token in the credential field, not the URL.";
  }

  return errors;
}

export interface ProjectCreateFormProps {
  /** Closes the panel; the page owns whether it is open. */
  onCancel: () => void;
}

export function ProjectCreateForm({ onCancel }: ProjectCreateFormProps) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();

  const [name, setName] = useState("");
  const [remoteUrl, setRemoteUrl] = useState("");
  const [defaultBranch, setDefaultBranch] = useState("");
  const [credential, setCredential] = useState("");
  const [fieldErrors, setFieldErrors] = useState<FieldErrors>({});

  const { submit, loading, error, clearError } = useFormSubmit(async () => {
    const branch = defaultBranch.trim();
    const token = credential;
    let created: Project;
    try {
      created = await createProject({
        name: name.trim(),
        remote_url: remoteUrl.trim(),
        ...(branch === "" ? {} : { default_branch: branch }),
        ...(token === "" ? {} : { credential: token }),
      });
    } catch (caught) {
      // Whatever happened, the token goes now and the user retypes it.
      setCredential("");
      // 409 is always the unique name; it belongs on the field, not in the
      // banner. Anything else is `useFormSubmit`'s to render.
      if (caught instanceof ApiError && caught.status === 409) {
        setFieldErrors({ name: caught.error });
        return;
      }
      throw caught;
    }

    setCredential("");
    setName("");
    setRemoteUrl("");
    setDefaultBranch("");

    // The detail view reads the same key, so it opens on the clone already in
    // flight instead of a spinner.
    queryClient.setQueryData(queryKeys.projects.detail(created.id), created);
    await queryClient.invalidateQueries({ queryKey: queryKeys.projects.all });
    void navigate(`/projects/${created.id}`);
  });

  function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const invalid = validateProjectForm(name, remoteUrl);
    setFieldErrors(invalid);
    if (invalid.name !== undefined || invalid.remoteUrl !== undefined) {
      return;
    }
    clearError();
    void submit();
  }

  return (
    <form
      onSubmit={onSubmit}
      aria-label="New project"
      noValidate
      className="border-console-border bg-console-surface grid gap-3 rounded border p-3 sm:grid-cols-2"
    >
      <FormField
        label="Name"
        name="project-name"
        value={name}
        onChange={(next) => {
          setName(next);
          setFieldErrors((current) => ({ ...current, name: undefined }));
        }}
        error={fieldErrors.name}
        autoComplete="off"
        autoFocus
        required
        disabled={loading}
      />

      <FormField
        label="Remote URL"
        name="project-remote-url"
        value={remoteUrl}
        onChange={(next) => {
          setRemoteUrl(next);
          setFieldErrors((current) => ({ ...current, remoteUrl: undefined }));
        }}
        error={fieldErrors.remoteUrl}
        hint="https://host/owner/repo.git"
        autoComplete="off"
        required
        disabled={loading}
      />

      <FormField
        label="Default branch"
        name="project-default-branch"
        value={defaultBranch}
        onChange={setDefaultBranch}
        hint="Optional; discovered from the remote when left empty."
        autoComplete="off"
        disabled={loading}
      />

      <FormField
        label="Credential"
        name="project-credential"
        type="password"
        value={credential}
        onChange={setCredential}
        hint="Personal access token for a private repository; stored write-only as the project secret GIT_CREDENTIAL and never shown again"
        autoComplete="off"
        disabled={loading}
      />

      {error !== null && (
        <div className="sm:col-span-2">
          <Alert kind="error" onDismiss={clearError}>
            {error}
          </Alert>
        </div>
      )}

      <div className="flex items-center justify-end gap-2 sm:col-span-2">
        <SubmitButton
          type="button"
          variant="ghost"
          disabled={loading}
          onClick={onCancel}
        >
          Cancel
        </SubmitButton>
        <SubmitButton loading={loading}>Start clone</SubmitButton>
      </div>
    </form>
  );
}
