# Architecture decision records

One file per decision where a real alternative was considered and rejected. Each record is short: context, decision, consequences. Records are never edited after acceptance except to add a "Superseded by" line; a changed decision is a new record.

Format: `NNNN-short-title.md`, status one of `accepted`, `superseded`.

| ADR | Title | Status |
| --- | --- | --- |
| [0001](0001-session-clones-not-worktrees.md) | Session clones instead of git worktrees | accepted |
| [0002](0002-github-pat-behind-credential-trait.md) | GitHub PAT behind a credential-provider trait | accepted |
| [0003](0003-long-lived-process-plus-resume.md) | Long-lived CLI process, resume for parked sessions | accepted |
| [0004](0004-rootless-podman-compat-api.md) | Rootless Podman through the Docker-compatible API | accepted |
| [0005](0005-listen-notify-as-wake-signal.md) | LISTEN/NOTIFY is a wake signal, rows are the truth | accepted |
| [0006](0006-envelope-encryption-in-orchestrator.md) | Envelope encryption in the orchestrator | accepted |
| [0007](0007-orchestrator-side-git.md) | Remote git operations only in the orchestrator, via MCP | accepted |
| [0008](0008-normalised-event-schema.md) | Normalised `AgentEvent` schema | accepted |
| [0009](0009-single-writer-task-leases.md) | Single-writer task claiming with leases | accepted |
| [0010](0010-transcript-file-as-recovery-source.md) | Transcript file, not the attach stream, is the recovery source | accepted |
| [0011](0011-shell-out-to-git.md) | Shell out to the `git` binary | accepted |
| [0012](0012-container-is-the-permission-boundary.md) | The container is the permission boundary | accepted |
| [0013](0013-invite-only-users.md) | Invite-only users with a seeded administrator | accepted |
| [0014](0014-email-via-resend.md) | Email through Resend behind an `EmailClient` trait | accepted |
