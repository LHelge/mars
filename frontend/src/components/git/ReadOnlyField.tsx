// A field the form does not ask about: the source of a session row's merge,
// the ref a push sends, the branch a rebase moves. It reads as a field so the
// grid beside it lines up, but there is nothing to focus and nothing to
// describe, so it is a label and a value rather than a `FieldShell`.

export interface ReadOnlyFieldProps {
  label: string;
  /** Truncated rather than wrapped: a ref is one line in this layout. */
  value: string;
}

export function ReadOnlyField({ label, value }: ReadOnlyFieldProps) {
  return (
    <div className="flex flex-col gap-1.5">
      <span className="text-console-muted text-xs">{label}</span>
      <p className="text-console-text truncate py-1.5 font-mono text-sm">
        {value}
      </p>
    </div>
  );
}
