// The console's control styling, in one place (`CLAUDE.md`, "Frontend
// conventions": shared UI).
//
// Every input, select and textarea in the app is the same bordered, monospace
// control; it used to be a class string copied into fifteen files and four
// differently-drifted constants. Both of these are plain strings, so a control
// that genuinely needs more — a width cap, a raised background — appends to
// one of them rather than starting a new copy.

/**
 * The control itself. `aria-invalid:border-state-failed` is why a control that
 * takes `FieldShell`'s `aria-invalid` turns red on an error without the caller
 * doing anything, and `placeholder:` is inert on a control with no placeholder.
 */
export const CONTROL =
  "border-console-border bg-console-bg text-console-text placeholder:text-console-muted aria-invalid:border-state-failed rounded border px-2.5 py-1.5 font-mono text-sm disabled:opacity-50";

/** The same control where it fills the column it sits in. */
export const FIELD = `${CONTROL} w-full`;
