// "Move to": the state a hand-off decision moves the task into (`SPEC.md`,
// "Code hand-offs and review": a forward or a revision carries a target state,
// and it has to be a different one).
//
// The same control in the review form and the revision form. The state the
// task is already in is not offered at all — a hand-off that does not move the
// task is refused — and the options come from the board's snapshot, so the
// form can only propose what the board can see.

import { FieldShell } from "../components/FieldShell";
import { FIELD } from "../components/fieldStyles";
import type { Task } from "../types";
import { STATE_HINT } from "./handoffRules";
import { useTaskStore } from "./taskStore";

export interface TargetStateSelectProps {
  /** The control's `name`; each form names its own. */
  id: string;
  task: Task;
  value: string;
  onChange: (next: string) => void;
  disabled: boolean;
}

export function TargetStateSelect({
  id,
  task,
  value,
  onChange,
  disabled,
}: TargetStateSelectProps) {
  const states = useTaskStore((store) => store.states);

  return (
    <FieldShell label="Move to" name={id} hint={STATE_HINT}>
      {(control) => (
        <select
          {...control}
          value={value}
          disabled={disabled}
          onChange={(event) => {
            onChange(event.target.value);
          }}
          className={FIELD}
        >
          <option value="">Choose a state</option>
          {states
            .filter((row) => row.name !== task.state)
            .map((row) => (
              <option key={row.id} value={row.name}>
                {row.name}
              </option>
            ))}
        </select>
      )}
    </FieldShell>
  );
}
