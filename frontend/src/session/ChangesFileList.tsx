// What changed, before how it changed (`SPEC.md`, "Frontend", "Changes
// panel"): the list of paths with their status and line counts, above the
// patch itself.
//
// The list is the panel's table of contents — a reviewer reads the file names
// first and goes to the one they care about — so a row is a link into the
// patch below rather than a static line.

import { EmptyState } from "../components";
import type { Diff } from "../types";

/** The letters `git diff --name-status` prints (`SPEC.md`, "Git"). */
const STATUS_TINT: Record<string, string> = {
  A: "text-state-running",
  M: "text-console-accent",
  D: "text-state-failed",
  R: "text-state-parked",
  C: "text-state-parked",
  T: "text-state-parked",
};

const STATUS_LABEL: Record<string, string> = {
  A: "added",
  M: "modified",
  D: "deleted",
  R: "renamed",
  C: "copied",
  T: "type changed",
};

/** What the endpoint says it cut the patch at (`SPEC.md`, "Git"). */
const TRUNCATED = "Patch truncated at 1 MiB; file counts are complete";

export interface ChangesFileListProps {
  diff: Diff;
  /**
   * Paths the patch showed as binary. Their counts read `bin`: the endpoint
   * reports zero and zero for a file git refused to diff, which a mode-only
   * change reports too, so the patch is what tells them apart.
   */
  binaryPaths?: ReadonlySet<string>;
  /** Called with the path of a clicked row, to scroll its hunks into view. */
  onSelect?: (path: string) => void;
}

export function ChangesFileList({
  diff,
  binaryPaths,
  onSelect,
}: ChangesFileListProps) {
  const additions = diff.files.reduce((sum, file) => sum + file.additions, 0);
  const deletions = diff.files.reduce((sum, file) => sum + file.deletions, 0);

  return (
    <section className="space-y-2">
      <div className="text-console-muted flex items-baseline gap-2 font-mono text-xs">
        <span>
          {diff.files.length} {diff.files.length === 1 ? "file" : "files"}
        </span>
        <span className="text-state-running">+{additions}</span>
        <span className="text-state-failed">−{deletions}</span>
      </div>

      {diff.truncated && (
        <p className="text-state-parked font-mono text-xs">{TRUNCATED}</p>
      )}

      {diff.files.length === 0 ? (
        <EmptyState title={`No changes against ${diff.base}`} />
      ) : (
        <ul className="border-console-border divide-console-border divide-y rounded border">
          {diff.files.map((file) => {
            const binary = binaryPaths?.has(file.path) ?? false;
            return (
              <li key={`${file.status}:${file.path}`}>
                <button
                  type="button"
                  onClick={() => onSelect?.(file.path)}
                  className="hover:bg-console-raised/40 flex w-full items-baseline gap-2 px-2 py-1 text-left font-mono text-xs"
                >
                  <span
                    title={STATUS_LABEL[file.status] ?? file.status}
                    className={`w-3 shrink-0 ${
                      STATUS_TINT[file.status] ?? "text-console-muted"
                    }`}
                  >
                    {file.status}
                  </span>
                  <span
                    title={file.path}
                    className="text-console-text min-w-0 flex-1 truncate"
                  >
                    {file.path}
                  </span>
                  {binary ? (
                    <span className="text-console-muted shrink-0">bin</span>
                  ) : (
                    <span className="shrink-0 tabular-nums">
                      <span className="text-state-running">
                        +{file.additions}
                      </span>{" "}
                      <span className="text-state-failed">
                        −{file.deletions}
                      </span>
                    </span>
                  )}
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
