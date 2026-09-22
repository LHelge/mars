// The project's queue states, which is what an agent profile may serve
// (`SPEC.md`, "Agent profiles": `serves_states` names queue states).
//
// One definition of the read, two readers: the served-states fieldset renders
// it, and the editor's role-template pre-fill drops a template's states this
// project has no queue state for. They share a query key, so it is one request
// either way, and neither can filter for `queue` differently from the other.

import { useQuery } from "@tanstack/react-query";

import { queryKeys } from "../../services/queryKeys";
import { listTaskStates } from "../../services/taskStates";
import type { TaskState } from "../../types";

export interface QueueStates {
  /** The read itself, for the pending, failed and empty distinctions. */
  query: ReturnType<typeof useTaskStatesQuery>;
  states: TaskState[];
}

function useTaskStatesQuery(projectId: string) {
  return useQuery({
    queryKey: queryKeys.projects.taskStates(projectId),
    queryFn: () => listTaskStates(projectId),
  });
}

export function useQueueStates(projectId: string): QueueStates {
  const query = useTaskStatesQuery(projectId);
  return {
    query,
    states: (query.data ?? []).filter((state) => state.kind === "queue"),
  };
}
