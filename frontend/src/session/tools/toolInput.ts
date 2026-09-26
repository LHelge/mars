// What the transcript may assume about a tool's `input` and `result`.
//
// Both are `unknown`: they are whatever the agent sent, and a CLI version can
// rename a field without telling us. Every renderer narrows through a guard
// here and falls back to the JSON tree when the guard says no, so an
// unexpected shape is shown rather than crashing the transcript.

export interface EditInput {
  file_path: string;
  old_string: string;
  new_string: string;
}

export interface MultiEditInput {
  file_path: string;
  edits: { old_string: string; new_string: string }[];
}

export interface WriteInput {
  file_path: string;
  content: string;
}

export interface NotebookEditInput {
  notebook_path: string;
  new_source: string;
  cell_id?: string;
}

export interface ShellInput {
  command: string;
  description?: string;
}

function record(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function str(value: unknown): value is string {
  return typeof value === "string";
}

export function isEditInput(input: unknown): input is EditInput {
  const fields = record(input);
  return (
    fields !== null &&
    str(fields.file_path) &&
    str(fields.old_string) &&
    str(fields.new_string)
  );
}

export function isMultiEditInput(input: unknown): input is MultiEditInput {
  const fields = record(input);
  if (
    fields === null ||
    !str(fields.file_path) ||
    !Array.isArray(fields.edits)
  ) {
    return false;
  }
  return fields.edits.every((edit) => {
    const one = record(edit);
    return one !== null && str(one.old_string) && str(one.new_string);
  });
}

export function isWriteInput(input: unknown): input is WriteInput {
  const fields = record(input);
  return fields !== null && str(fields.file_path) && str(fields.content);
}

export function isNotebookEditInput(
  input: unknown,
): input is NotebookEditInput {
  const fields = record(input);
  return (
    fields !== null &&
    str(fields.notebook_path) &&
    str(fields.new_source) &&
    (fields.cell_id === undefined || str(fields.cell_id))
  );
}

export function isShellInput(input: unknown): input is ShellInput {
  const fields = record(input);
  return (
    fields !== null &&
    str(fields.command) &&
    (fields.description === undefined || str(fields.description))
  );
}

/** A string field of an input object, or `undefined` if it is not one. */
export function inputString(input: unknown, key: string): string | undefined {
  const fields = record(input);
  const value = fields?.[key];
  return str(value) ? value : undefined;
}

/** A number field of an input object, or `undefined` if it is not one. */
export function inputNumber(input: unknown, key: string): number | undefined {
  const fields = record(input);
  const value = fields?.[key];
  return typeof value === "number" ? value : undefined;
}

/**
 * `tool_result.content` as text, or `null` when it is not text at all and
 * belongs in the JSON tree. A Claude tool result is either a string or an
 * array of content blocks; an array of text blocks is joined with newlines,
 * and an array holding anything else (an image, say) is not flattened.
 */
export function resultText(result: unknown): string | null {
  if (result === undefined || result === null) {
    return null;
  }
  if (typeof result === "string") {
    return result;
  }
  if (Array.isArray(result)) {
    const texts: string[] = [];
    for (const block of result) {
      const fields = record(block);
      if (fields === null || fields.type !== "text" || !str(fields.text)) {
        return null;
      }
      texts.push(fields.text);
    }
    return texts.join("\n");
  }
  return null;
}

/** The one-line form of a read, glob, grep, list or web tool call. */
export function summaryLine(name: string, input: unknown): string {
  const key = name.toLowerCase();
  if (key === "read") {
    const path = inputString(input, "file_path") ?? "";
    const offset = inputNumber(input, "offset");
    const limit = inputNumber(input, "limit");
    // `limit` is a count of lines, not the last one: offset 100 with limit 50
    // reads lines 100 to 150, and printing it as `100-50` read like a range
    // that runs backwards.
    const start = offset ?? 0;
    const range =
      offset === undefined && limit === undefined
        ? ""
        : ` :${String(start)}-${limit === undefined ? "" : String(start + limit)}`;
    return `Read ${path}${range}`;
  }
  if (key === "glob" || key === "grep") {
    const pattern = inputString(input, "pattern") ?? "";
    const path = inputString(input, "path");
    const where = path === undefined ? "" : ` in ${path}`;
    return `${name} ${pattern}${where}`;
  }
  if (key === "ls") {
    return `LS ${inputString(input, "path") ?? ""}`;
  }
  if (key === "webfetch") {
    return `WebFetch ${inputString(input, "url") ?? ""}`;
  }
  if (key === "websearch") {
    return `WebSearch ${inputString(input, "query") ?? ""}`;
  }
  return name;
}

/**
 * What a collapsed tool row says about its call, beside the tool's name: the
 * command, the file or the pattern. Empty when the input has nothing of the
 * kind, and for a subagent, whose group row already carries its description.
 */
export function headerSummary(name: string, input: unknown): string {
  if (isShellInput(input)) {
    // `split` with a limit of 1 always yields one element, even for "".
    return input.description ?? input.command.split("\n", 1)[0] ?? "";
  }
  const path =
    inputString(input, "file_path") ?? inputString(input, "notebook_path");
  if (path !== undefined) {
    return path;
  }
  const line = summaryLine(name, input);
  return line === name ? "" : line.slice(name.length).trim();
}
