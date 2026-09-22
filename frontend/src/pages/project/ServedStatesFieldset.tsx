// The queue states an agent profile picks work up from (`SPEC.md`, "Agent
// profiles": `serves_states`).
//
// It reads the project's states itself, because what it offers is exactly what
// that read answers. Three outcomes are told apart on purpose: a read still
// running, a read that failed — which must never be shown as "this project has
// no queue states", since that invites clearing a profile down to serving
// nothing (`SPEC.md`, "Frontend", Read failures) — and a project that really
// has none.

import { QueryErrorAlert } from "../../components/QueryErrorAlert";
import { CheckboxList, Fieldset } from "./profileFields";
import { useQueueStates } from "./queueStates";

export interface ServedStatesFieldsetProps {
  projectId: string;
  /** The form's `serves_states`. */
  selected: string[];
  onToggle: (name: string) => void;
  disabled: boolean;
  /**
   * Whether the dispatcher launches this profile by itself — an ephemeral
   * profile with `auto_launch` — which is the one case where served states
   * also decide what gets started, not only what the agent is offered.
   */
  dispatched: boolean;
}

export function ServedStatesFieldset({
  projectId,
  selected,
  onToggle,
  disabled,
  dispatched,
}: ServedStatesFieldsetProps) {
  const { query, states } = useQueueStates(projectId);

  // A state that was renamed or deleted since the profile was saved would be a
  // 400 on submit; show it so it can be cleared — but only once the project's
  // own list has actually arrived, or every served state would be flagged on a
  // cold cache.
  const orphans = (query.isSuccess ? selected : []).filter(
    (name) => !states.some((state) => state.name === name),
  );

  return (
    <Fieldset
      legend="Served states"
      description={
        dispatched
          ? "The dispatcher watches these queue states and, within the caps, launches a session of this profile for each unheld, unblocked task in them, holding that task from the start."
          : "Queue states this profile picks work up from: the agent's ready tool lists the unheld, unblocked tasks in them, and it can claim only those. A launch for a named task works whatever its state."
      }
      help="task-flow"
    >
      {query.isError && (
        <div className="pb-2">
          <QueryErrorAlert
            query={query}
            message="Could not load this project's task states."
          />
        </div>
      )}

      {states.length === 0 ? (
        query.isPending ? (
          <p className="text-console-muted text-xs">Loading states…</p>
        ) : (
          query.isSuccess && (
            <p className="text-console-muted text-xs">
              This project has no queue states.
            </p>
          )
        )
      ) : (
        <div className="flex flex-wrap gap-x-4 gap-y-2">
          <CheckboxList
            items={states.map((state) => ({ name: state.name }))}
            selected={selected}
            onToggle={onToggle}
            disabled={disabled}
          />
        </div>
      )}

      <CheckboxList
        items={orphans.map((name) => ({
          name,
          note: <span className="font-sans">not a queue state</span>,
          className:
            "text-state-parked flex items-center gap-2 pt-2 font-mono text-xs",
        }))}
        selected={selected}
        onToggle={onToggle}
        disabled={disabled}
      />
    </Fieldset>
  );
}
