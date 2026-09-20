// The 422 of a merge or a rebase: git stopped, nothing was written, and these
// are the files to look at (`SPEC.md`, "Git"). The paths are the answer, so
// they are the content — monospace, one per line, no truncation.

export interface ConflictListProps {
  paths: string[];
  /** The server's sentence, shown above the paths when there is one. */
  message?: string;
}

export function ConflictList({ paths, message }: ConflictListProps) {
  return (
    <div
      role="alert"
      className="border-state-parked/60 bg-console-surface rounded border px-3 py-2 text-sm"
    >
      <p className="text-state-parked">Conflicts in:</p>
      <ul className="text-console-text mt-1 space-y-0.5 font-mono text-xs">
        {paths.map((path) => (
          <li key={path} className="break-all">
            {path}
          </li>
        ))}
      </ul>
      {message !== undefined && message !== "" && (
        <p className="text-console-muted mt-1 text-xs">{message}</p>
      )}
    </div>
  );
}
