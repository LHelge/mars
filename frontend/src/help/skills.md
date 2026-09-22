Skills are instructions and scripts that the agent CLI loads on demand. Sessions can load them from two places, and every session of a project already shares both. You don't need a shared directory for them.

### In the repository (recommended)

A skill committed at `.claude/skills/<name>/SKILL.md` loads in every session whose checkout contains it. This is the best place for a skill. It is versioned and reviewed with the code, and each session sees the skills of the branch it started from. Plugins that the repository enables in `.claude/settings.json` arrive the same way.

This is also where the repository's other instructions live. Its `CLAUDE.md` and `.mcp.json` are read in every session, beside the profile's [system prompt](help:profiles).

### In the project's CLI state directory

Each project has one agent CLI state directory, and every session of that project uses it as its `~/.claude`. A skill placed at `skills/<name>/SKILL.md` in there is available to every session of that project, and to no other project. Use it for skills that don't belong in the repository.

There are two ways to put a skill there:

- **From a session.** An agent can install into the directory, for example a plugin installed with `claude plugin install`. Every other session of the project then loads it too. Sessions of one project trust each other.
- **On the host** (needs host access). Place the files under `DATA_DIR_HOST/projects/<project id>/claude/skills/`, owned by the user the orchestrator runs as.

`/session/home/.claude` is **not** read. Each session's home directory is its own, and the CLI's state directory takes the place of `~/.claude`. A skill placed there is never loaded.

### When a new skill takes effect

A skill is picked up the next time the CLI starts: in a new session, or when a parked session resumes. A session that is already running keeps the skills it started with.

Mars doesn't show which skills a session loaded. With host access, you can find them listed at the start of the session's raw log, `DATA_DIR_HOST/sessions/<session id>/log/stream.jsonl`. Deleting the project deletes its CLI state directory and everything installed in it.
