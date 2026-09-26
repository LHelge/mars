# 0054. A session clone borrows through `--shared` alone, recording the mirror as `DATA_DIR` spells it

Status: accepted (Bears task `d6vuy`). Amends the command line of ADR 0001; the reference-clone model it chose is unchanged.

## Context

ADR 0001 creates each session's work clone with `git clone --reference <mirror>`, and `ARCHITECTURE.md` added `--shared` because `--reference` on a local path still copies the whole object directory. The two flags name the same repository, and on a `DATA_DIR` without symlinks they write the same alternates line, which git then does not repeat.

They do not write it the same way. `--shared` records the source path as it was given; `--reference` records the reference by its real path, every symlink resolved (observed on git 2.55; the same on Linux and macOS). Under a `DATA_DIR` reached through a symlink — every macOS temporary directory, since `/var` is a link to `/private/var`, and any host where the data directory is a link — the alternates file gets both: the resolved path and the spelled one.

The session container mounts the mirror at the orchestrator's own spelling of the path. The resolved line names nothing there. Git tolerates it, because the other line still finds the objects, but every command the agent runs that reads objects prints `error: unable to normalize alternate object path` for it (observed on git 2.55).

## Decision

The work clone is `git clone --shared --no-checkout --no-hardlinks <mirror> <work>`, with no `--reference`. Its alternates file holds exactly one line: the mirror's objects directory as `DATA_DIR` spells it.

Rejected:

- **Canonicalising the paths handed to git** (`std::fs::canonicalize` on the mirror path in `src/git/`). Both flags would then agree, but on the resolved path, which is not the path the mirror is mounted at in the container — the one place the alternates line has to resolve.
- **Making the test compare the set of canonicalised lines.** It would pass on macOS and leave the second, dead line in every session clone there.

## Consequences

- A session clone's alternates file is the same on every host: one line, the path the container mount uses.
- The merge and rebase temporary clones already used `--shared` alone, so every clone the orchestrator makes now borrows the same way.
