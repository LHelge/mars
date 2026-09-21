# Architecture decision records

One file per decision where a real alternative was considered and rejected. Each record is short: context with the options considered, decision, consequences, under 300 words. A record is the why; the rule it establishes always lives in the main document (`SPEC.md`, `ARCHITECTURE.md`, `docs/data-model.md`, `README.md`), so nobody has to read a record to know what to build. Records are never edited after acceptance except to add a "Superseded by" line or to condense wording without changing the decision; a changed decision is a new record. A record over the cap is condensed, never truncated: the rejected alternatives and the consequences stay.

Format: `NNNN-short-title.md`, status one of `accepted`, `superseded`.

| ADR | Title | Status |
| --- | --- | --- |
| [0001](0001-session-clones-not-worktrees.md) | Session clones instead of git worktrees | accepted |
| [0002](0002-github-pat-behind-credential-trait.md) | GitHub PAT behind a credential-provider trait | accepted |
| [0003](0003-long-lived-process-plus-resume.md) | Long-lived CLI process, resume for parked sessions | accepted |
| [0004](0004-rootless-podman-compat-api.md) | Rootless Podman through the Docker-compatible API | accepted |
| [0005](0005-listen-notify-as-wake-signal.md) | LISTEN/NOTIFY is a wake signal, rows are the truth | accepted; transaction ordering superseded by 0028 |
| [0006](0006-envelope-encryption-in-orchestrator.md) | Envelope encryption in the orchestrator | accepted |
| [0007](0007-orchestrator-side-git.md) | Remote git operations only in the orchestrator, via MCP | accepted |
| [0008](0008-normalised-event-schema.md) | Normalised `AgentEvent` schema | accepted |
| [0009](0009-single-writer-task-leases.md) | Single-writer task claiming with leases | superseded by 0016 |
| [0010](0010-transcript-file-as-recovery-source.md) | Transcript file, not the attach stream, is the recovery source | accepted |
| [0011](0011-shell-out-to-git.md) | Shell out to the `git` binary | accepted |
| [0012](0012-container-is-the-permission-boundary.md) | The container is the permission boundary | accepted; containment claim qualified by 0019 |
| [0013](0013-invite-only-users.md) | Invite-only users with a seeded administrator | accepted |
| [0014](0014-email-via-resend.md) | Email through Resend behind an `EmailClient` trait | accepted |
| [0015](0015-project-shared-directories.md) | Project-scoped shared directories and a per-project CLI state directory | accepted |
| [0016](0016-task-state-as-queue.md) | Task state is a project-defined queue; leases follow session liveness | accepted |
| [0017](0017-separate-upstream-and-integration-refs.md) | Separate upstream tracking from Mars integration branches | accepted |
| [0018](0018-commit-bound-task-handoffs.md) | Task hand-offs retain a commit; review approval belongs to that commit | accepted |
| [0019](0019-defer-isolation-of-git-checkout-operations.md) | Defer isolation of git operations on agent-controlled checkouts | accepted; known v1 vulnerability |
| [0020](0020-defer-durable-input-delivery.md) | Accept input-delivery uncertainty across orchestrator restarts in v1 | accepted; known v1 limitation |
| [0021](0021-serialize-tracker-mutations-per-project.md) | Serialize tracker mutations per project | accepted |
| [0022](0022-refresh-task-board-on-events.md) | Refresh the task board on events and retain deleted task identities | accepted |
| [0023](0023-define-tracker-edge-cases.md) | Define tracker edge cases without adding workflow machinery | accepted |
| [0024](0024-retain-operator-controlled-admin-bootstrap.md) | Retain the fixed administrator credentials for controlled setup | accepted |
| [0025](0025-check-current-user-and-revoke-logins.md) | Check current user authority and revoke logins on password changes | accepted |
| [0026](0026-allow-email-fallback-token-logging.md) | Intentionally log invitation and reset links when email is unconfigured | accepted |
| [0027](0027-defer-transcript-secret-redaction.md) | Preserve agent output without automatic secret redaction in v1 | accepted |
| [0028](0028-notify-within-event-transactions.md) | Issue notifications within event transactions | accepted |
| [0029](0029-mcp-token-per-process-launch.md) | Generate a fresh MCP token for each session process launch | accepted |
| [0030](0030-record-tracker-changes-not-reads.md) | Record tracker changes, not read-only tool calls | accepted |
| [0031](0031-local-task-board-search.md) | Filter the task board locally by title or number | accepted |
| [0032](0032-run-state-on-stdin-attach.md) | A session is `running` when stdin is attached, not when `init` arrives | accepted |
| [0033](0033-no-interactive-prompts-in-v1.md) | No interactive prompts: the agent never asks the host a question | accepted |
| [0034](0034-cli-stdin-is-a-fifo-fed-by-an-exec.md) | The CLI's stdin is a FIFO it holds open itself, fed through an exec | accepted |
| [0035](0035-compose-engine-variants-as-override-files.md) | Carry the Podman/Docker compose difference in one-line override files | accepted |
| [0036](0036-agent-credentials-belong-to-the-backend.md) | Agent credentials belong to the backend and are resolved implicitly | accepted |
| [0037](0037-nextest-runs-the-backend-suite.md) | `cargo nextest` runs the backend suite; the live engine suites keep `cargo test` | accepted |
| [0038](0038-seed-role-profiles-at-project-creation.md) | Seed the four role profiles at project creation, copied not referenced | accepted |
| [0039](0039-layered-dev-session-image.md) | A layered dev session image on top of the contract base | accepted |
| [0040](0040-render-markdown-in-the-browser.md) | Markdown is rendered in the browser from text; no server-side HTML | accepted |
