// `POST /projects/{pid}/shared-dirs` (`SPEC.md`, "Shared directories"): the
// name of a directory on disk and the path every session container mounts it
// at.
//
// Both fields are checked here before the request goes out, with the same
// rules and in the same order as the orchestrator, so a path that can never be
// mounted is answered while the cursor is still in the field. The server stays
// the authority: its 400 and its two 409s — the name is taken, the path is
// taken — replace whatever the local check said.
//
// The presets are the per-ecosystem starting points of `README.md`, "Operating
// notes". They fill the form rather than submitting it, so a name can be
// adjusted before it becomes a directory.

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import type { FormEvent } from "react";
import { Alert } from "../../components/Alert";
import { FormField } from "../../components/FormField";
import { SubmitButton } from "../../components/SubmitButton";
import { errorMessage, logUnexpected } from "../../services/errorMessage";
import { queryKeys } from "../../services/queryKeys";
import { createSharedDir } from "../../services/projects";
import type { SharedDir, SharedDirInput } from "../../types";
import { SHARED_DIR_PRESETS, validateSharedDir } from "../../utils/sharedDir";
import { sharedDirErrorField } from "./sharedDirMessages";

export interface SharedDirFormProps {
  projectId: string;
}

export function SharedDirForm({ projectId }: SharedDirFormProps) {
  const queryClient = useQueryClient();
  const listKey = queryKeys.projects.sharedDirs(projectId);

  const [name, setName] = useState("");
  const [containerPath, setContainerPath] = useState("");
  const [nameError, setNameError] = useState<string | null>(null);
  const [pathError, setPathError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const create = useMutation({
    mutationFn: (input: SharedDirInput) => createSharedDir(projectId, input),
    onSuccess: (created: SharedDir) => {
      queryClient.setQueryData<SharedDir[]>(listKey, (rows) =>
        rows === undefined ? [created] : [...rows, created],
      );
      void queryClient.invalidateQueries({ queryKey: listKey });
      setName("");
      setContainerPath("");
      setError(null);
    },
    onError: (caught: unknown) => {
      logUnexpected(caught);
      const message = errorMessage(caught);
      const field = sharedDirErrorField(caught);
      // A refusal that names a field belongs beside that field; anything else
      // goes above the form, where nothing else can explain it.
      setNameError(field === "name" ? message : null);
      setPathError(field === "containerPath" ? message : null);
      setError(field === null ? message : null);
    },
  });

  function applyPreset(preset: (typeof SHARED_DIR_PRESETS)[number]) {
    setName(preset.name);
    setContainerPath(preset.container_path);
    setNameError(null);
    setPathError(null);
    setError(null);
  }

  function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const invalid = validateSharedDir(name, containerPath);
    setNameError(invalid.name);
    setPathError(invalid.containerPath);
    if (invalid.name !== null || invalid.containerPath !== null) {
      return;
    }
    setError(null);
    create.mutate({ name: name.trim(), container_path: containerPath.trim() });
  }

  return (
    <form
      onSubmit={onSubmit}
      aria-label="Add a shared directory"
      className="border-console-border bg-console-surface flex flex-col gap-3 rounded border p-3"
    >
      <div className="grid gap-3 sm:grid-cols-[minmax(10rem,1fr)_2fr]">
        <FormField
          label="Name"
          name="shared-dir-name"
          value={name}
          onChange={(next) => {
            setName(next);
            setNameError(null);
          }}
          error={nameError ?? undefined}
          hint="The directory name on disk."
          autoComplete="off"
          required
        />

        <FormField
          label="Container path"
          name="shared-dir-path"
          value={containerPath}
          onChange={(next) => {
            setContainerPath(next);
            setPathError(null);
          }}
          error={pathError ?? undefined}
          hint="Where every session container mounts it, read-write."
          autoComplete="off"
          required
        />
      </div>

      <div className="flex flex-wrap items-center gap-1.5">
        <span className="text-console-muted text-xs">Starting points</span>
        {SHARED_DIR_PRESETS.map((preset) => (
          <button
            key={preset.name}
            type="button"
            onClick={() => {
              applyPreset(preset);
            }}
            title={`${preset.ecosystem}: ${preset.container_path}`}
            className="border-console-border text-console-muted hover:text-console-text hover:border-console-accent rounded border px-2 py-0.5 font-mono text-xs"
          >
            {preset.name}
          </button>
        ))}
      </div>

      {error !== null && <Alert kind="error">{error}</Alert>}

      <div className="flex justify-end">
        <SubmitButton loading={create.isPending}>Add directory</SubmitButton>
      </div>
    </form>
  );
}
