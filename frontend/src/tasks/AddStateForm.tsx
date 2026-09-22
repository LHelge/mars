// Adding a column to the board (`SPEC.md`, "Task states": `POST
// /projects/{pid}/task-states`).
//
// Three fields on one line, because that is what adding a state is: a name,
// what the orchestrator may do with it, and where it goes. An empty position
// appends, which the hint says rather than leaving the field to be guessed at,
// and the `human` option closes itself once the project has one — the server
// answers 409 for a second, and the reason is knowable from the list on
// screen.

import { useState } from "react";
import type { FormEvent } from "react";

import { Alert } from "../components/Alert";
import { FieldShell } from "../components/FieldShell";
import { CONTROL } from "../components/fieldStyles";
import { FormField } from "../components/FormField";
import { SubmitButton } from "../components/SubmitButton";
import { useFormSubmit } from "../hooks/useFormSubmit";
import { createTaskState } from "../services/taskStates";
import { parseTaskStateKind, TASK_STATE_KINDS } from "../types";
import type { TaskStateKind } from "../types";
import { HUMAN_TAKEN, KIND_MEANING, stateNameError } from "./taskStateRules";

export interface AddStateFormProps {
  projectId: string;
  hasHuman: boolean;
  stateCount: number;
  afterMutation: () => Promise<void>;
}

export function AddStateForm({
  projectId,
  hasHuman,
  stateCount,
  afterMutation,
}: AddStateFormProps) {
  const [name, setName] = useState("");
  const [kind, setKind] = useState<TaskStateKind>("queue");
  const [position, setPosition] = useState("");
  const [nameError, setNameError] = useState<string | null>(null);
  const [positionError, setPositionError] = useState<string | null>(null);

  const add = useFormSubmit(async () => {
    await createTaskState(projectId, {
      name: name.trim(),
      kind,
      ...(position.trim() === "" ? {} : { position: Number(position) }),
    });
    await afterMutation();
    setName("");
    setKind("queue");
    setPosition("");
  });

  function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();

    const invalidName = stateNameError(name.trim());
    const trimmedPosition = position.trim();
    const invalidPosition =
      trimmedPosition !== "" && !/^\d+$/.test(trimmedPosition)
        ? "Position must be a whole number, counting from 0"
        : null;

    setNameError(invalidName);
    setPositionError(invalidPosition);
    if (invalidName !== null || invalidPosition !== null) {
      return;
    }

    void add.submit();
  }

  return (
    <form
      onSubmit={onSubmit}
      aria-label="Add a task state"
      className="border-console-border bg-console-surface flex flex-col gap-3 rounded border p-3"
    >
      <div className="grid gap-3 sm:grid-cols-[minmax(10rem,2fr)_minmax(8rem,1fr)_minmax(6rem,1fr)]">
        <FormField
          label="Name"
          name="task-state-name"
          value={name}
          onChange={(next) => {
            setName(next);
            setNameError(null);
          }}
          error={nameError ?? undefined}
          hint="What agents and the board call this column."
          autoComplete="off"
          required
        />

        <FieldShell label="Kind" name="task-state-kind" hint={KIND_MEANING[kind]}>
          {(control) => (
            <select
              {...control}
              value={kind}
              onChange={(event) => {
                // The options are `TASK_STATE_KINDS` itself, so nothing
                // else can arrive; an unknown value leaves the kind alone.
                const chosen = parseTaskStateKind(event.target.value);
                if (chosen !== undefined) {
                  setKind(chosen);
                }
              }}
              className={CONTROL}
            >
              {TASK_STATE_KINDS.map((option) => (
                <option
                  key={option}
                  value={option}
                  disabled={option === "human" && hasHuman}
                  title={option === "human" && hasHuman ? HUMAN_TAKEN : undefined}
                >
                  {option}
                  {option === "human" && hasHuman ? ` — ${HUMAN_TAKEN}` : ""}
                </option>
              ))}
            </select>
          )}
        </FieldShell>

        <FormField
          label="Position"
          name="task-state-position"
          type="number"
          value={position}
          onChange={(next) => {
            setPosition(next);
            setPositionError(null);
          }}
          error={positionError ?? undefined}
          hint={`Empty appends, at ${String(stateCount)}.`}
          autoComplete="off"
        />
      </div>

      {add.error !== null && <Alert kind="error">{add.error}</Alert>}

      <div className="flex justify-end">
        <SubmitButton loading={add.loading}>Add state</SubmitButton>
      </div>
    </form>
  );
}
