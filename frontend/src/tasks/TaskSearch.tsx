// The search field of `SPEC.md`, "Frontend", "Task-board search" (ADR 0031).
//
// One row of console chrome above the columns: the label, the field, and a
// clear action that only exists while there is something to clear. The query
// lives in the board store rather than here, so it survives the re-renders a
// live refresh causes and the moves between the board and a task's own route —
// and it changes only on a keystroke or a project change, so the field never
// loses the cursor to a refresh.

import type { RefObject } from "react";

import { CONTROL } from "../components/fieldStyles";
import { SubmitButton } from "../components/SubmitButton";
import { useTaskStore } from "./taskStore";

const SEARCH_INPUT_ID = "task-search";

export interface TaskSearchProps {
  /** Held by the board so its `No matching tasks` action can refocus too. */
  inputRef?: RefObject<HTMLInputElement | null>;
  /** Clears the query and returns the cursor to the field. */
  onClear: () => void;
}

export function TaskSearch({ inputRef, onClear }: TaskSearchProps) {
  const query = useTaskStore((state) => state.query);
  const setQuery = useTaskStore((state) => state.setQuery);

  return (
    <div className="flex flex-wrap items-center gap-2">
      <label
        htmlFor={SEARCH_INPUT_ID}
        className="text-console-muted shrink-0 text-xs"
      >
        Search
      </label>

      <input
        ref={inputRef}
        id={SEARCH_INPUT_ID}
        name={SEARCH_INPUT_ID}
        type="search"
        value={query}
        aria-label="Search tasks"
        placeholder="Search title or #number"
        autoComplete="off"
        spellCheck={false}
        onChange={(event) => {
          setQuery(event.target.value);
        }}
        // Full width below `sm`, beside its label; a capped field above it.
        className={`${CONTROL} min-w-0 flex-1 sm:w-full sm:max-w-72 sm:flex-none`}
      />

      {query !== "" && (
        <SubmitButton type="button" variant="ghost" onClick={onClear}>
          Clear search
        </SubmitButton>
      )}
    </div>
  );
}
