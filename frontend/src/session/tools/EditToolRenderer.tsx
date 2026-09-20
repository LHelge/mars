// Edit and write tools: what the file looked like before and after, as a diff
// (`SPEC.md`, "Transcript rendering"). A write and a notebook edit have no
// "before", so they render as a pure insertion, which is what they are.
//
// The tool result is a confirmation string; it stays below the diff so a
// failure is still readable where the change would have been.

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

function notice(oldText: string, newText: string): string | undefined {
  return lineDiffCapped(oldText, newText) ? CAPPED : undefined;
}

function Diffs({ message }: { message: ToolMessage }) {
  const input = message.input;

  if (isEditInput(input)) {
    return (
      <DiffView
        path={input.file_path}
        lines={lineDiff(input.old_string, input.new_string)}
        notice={notice(input.old_string, input.new_string)}
      />
    );
  }

  if (isMultiEditInput(input)) {
    return (
      <div className="space-y-2">
        {input.edits.map((edit, index) => (
          <DiffView
            key={index}
            path={
              input.edits.length > 1
                ? `${input.file_path} · edit ${index + 1} of ${input.edits.length}`
                : input.file_path
            }
            lines={lineDiff(edit.old_string, edit.new_string)}
            notice={notice(edit.old_string, edit.new_string)}
          />
        ))}
      </div>
    );
  }

  if (isWriteInput(input)) {
    return (
      <DiffView
        path={input.file_path}
        lines={lineDiff("", input.content)}
        notice={notice("", input.content)}
      />
    );
  }

  if (isNotebookEditInput(input)) {
    return (
      <DiffView
        path={
          input.cell_id === undefined
            ? input.notebook_path
            : `${input.notebook_path} · cell ${input.cell_id}`
        }
        lines={lineDiff("", input.new_source)}
        notice={notice("", input.new_source)}
      />
    );
  }

  return null;
}

export interface EditToolRendererProps {
  message: ToolMessage;
}

export function EditToolRenderer({ message }: EditToolRendererProps) {
  const input = message.input;
  const known =
    isEditInput(input) ||
    isMultiEditInput(input) ||
    isWriteInput(input) ||
    isNotebookEditInput(input);
  if (!known) {
    // A shape no guard recognises is still shown, just not as a diff.
    return <JsonToolRenderer message={message} />;
  }

  return (
    <div className="space-y-2">
      <Diffs message={message} />
      <ToolResult message={message} />
    </div>
  );
}
