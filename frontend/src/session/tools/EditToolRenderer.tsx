// Edit and write tools: what the file looked like before and after, as a diff
// (`SPEC.md`, "Transcript rendering"). A write and a notebook edit have no
// "before", so they render as a pure insertion, which is what they are.
//
// The tool result is a confirmation string; it stays below the diff so a
// failure is still readable where the change would have been.
//
// The alignment is an O(n·m) walk (`utils/diff.ts`), so it is computed once
// per pair of texts and not once per render: the input is read into the pairs
// it describes by one pass of the guards, and each pair is drawn by a
// `memo()`-wrapped `Diff` whose props are those two strings. A row re-renders
// for reasons that leave its input alone — a sibling streaming, the result
// landing, a disclosure opening — and none of them realign anything. Nothing
// in this build memoises for us (`ARCHITECTURE.md`, "Frontend architecture").

import { memo, useMemo } from "react";

import { DiffView } from "../../components/DiffView";
import { lineDiff, lineDiffCapped } from "../../utils/diff";
import type { ToolMessage } from "../sessionStore";
import { JsonToolRenderer } from "./JsonToolRenderer";
import { ToolResult } from "./ToolResult";
import {
  isEditInput,
  isMultiEditInput,
  isNotebookEditInput,
  isWriteInput,
} from "./toolInput";

/** Shown in place of an alignment the pair is too large for. */
const CAPPED =
  "Too large to align line by line; the old and the new text are shown in full.";

/** One diff to draw: the path it is labelled with and the two texts. */
interface DiffPair {
  path: string;
  oldText: string;
  newText: string;
}

/**
 * The diffs a tool input describes, or `null` for a shape no guard recognises.
 * One pass over the guards, so nothing asks the same question twice.
 */
function diffPairs(input: unknown): DiffPair[] | null {
  if (isEditInput(input)) {
    return [
      {
        path: input.file_path,
        oldText: input.old_string,
        newText: input.new_string,
      },
    ];
  }

  if (isMultiEditInput(input)) {
    const { edits, file_path: path } = input;
    return edits.map((edit, index) => ({
      path:
        edits.length > 1
          ? `${path} · edit ${index + 1} of ${edits.length}`
          : path,
      oldText: edit.old_string,
      newText: edit.new_string,
    }));
  }

  if (isWriteInput(input)) {
    return [{ path: input.file_path, oldText: "", newText: input.content }];
  }

  if (isNotebookEditInput(input)) {
    return [
      {
        path:
          input.cell_id === undefined
            ? input.notebook_path
            : `${input.notebook_path} · cell ${input.cell_id}`,
        oldText: "",
        newText: input.new_source,
      },
    ];
  }

  return null;
}

const Diff = memo(function Diff({ path, oldText, newText }: DiffPair) {
  // `memo` stops a re-render with the same props; this keeps the alignment
  // across one that has a different `path` — a multi-edit relabelled when
  // another edit of the same call arrives — where the texts are unchanged.
  const { lines, capped } = useMemo(
    () => ({
      lines: lineDiff(oldText, newText),
      capped: lineDiffCapped(oldText, newText),
    }),
    [oldText, newText],
  );

  return (
    <DiffView path={path} lines={lines} notice={capped ? CAPPED : undefined} />
  );
});

export interface EditToolRendererProps {
  message: ToolMessage;
}

export function EditToolRenderer({ message }: EditToolRendererProps) {
  const pairs = useMemo(() => diffPairs(message.input), [message.input]);

  if (pairs === null) {
    // A shape no guard recognises is still shown, just not as a diff.
    return <JsonToolRenderer message={message} />;
  }

  return (
    <div className="space-y-2">
      {/* The index is the key: a multi-edit's edits are a fixed list in the
          order the tool was called with, and nothing reorders them. */}
      {pairs.map((pair, index) => (
        <Diff key={index} {...pair} />
      ))}
      <ToolResult message={message} />
    </div>
  );
}
