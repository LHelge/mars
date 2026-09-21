# Verifying the claude-dev session image with real credentials

Manual and credentialed; never runs in CI. It is the live counterpart of
`images/claude/VERIFY.md`: that file proves the CLI contract on the base image,
this one proves that a seeded `implementer` on the **dev** image can actually
build and test a Rust and Node repository, and that the environment paragraph
of the role templates (`SPEC.md`, "Role profile templates") changes behaviour
when a tool really is missing.

Last run: **2026-09-21**, Claude Code **2.1.274** (the dev image inherits the
base's pin; `mars.claude_code_version` label), model **`claude-haiku-4-5`**,
Rust **1.98.1** in the image (`mars.rust_version`), rootless **Podman 6.1.2**,
image `localhost/mars-session-claude-dev:latest`. Four sessions, **0.5041 USD**
of reported cost in total (the `result` events' `cost_usd`). The credential is a
Claude Code OAuth token that lives only in the operator's own file
(`~/.config/mars/claude-oauth-token`) and in the shell that stores it; it is
never printed, never written to this file and never read back out of a
transcript (rule 3 of `CLAUDE.md`). Every dump this run produced was checked
against that file with `grep -c -F -f <token file> <dump>` — which must print
`0` — before anyone read it, because agent output is stored unredacted by
design (ADR 0027).

## Procedure

An orchestrator of the operator's own, on ports nobody else uses, with its own
Postgres, data directory and session networks, so it collides with nothing
already running on the machine.

```bash
# Build order and tags: README.md, "Session image". Nothing is rebuilt here if
# the tags already exist.
podman run -d --name mars-f67v8-pg -e POSTGRES_USER=mars -e POSTGRES_PASSWORD=mars \
  -e POSTGRES_DB=mars -p 5440:5432 postgres:18

# The four throwaway repositories, one per scenario, as bare repositories the
# orchestrator clones over the `file://` transport. Each holds a one-function
# Cargo crate, a `package.json` with a `node --test` script, a `.gitignore` and
# a `CLAUDE.md` naming the repository's checks. Scenario B adds a
# rust-toolchain.toml, C demands `cargo nextest run`, D demands an apt package.
# No GitHub repository is created and nothing is pushed anywhere.

cd orchestrator && cargo build --features integration-tests   # see the note below
API_PORT=7100 MCP_PORT=7101 SESSION_IMAGE_DEFAULT=localhost/mars-session-claude-dev:latest \
  SESSION_NETWORK_INTERNAL=mars-f67v8-sessions SESSION_NETWORK_EGRESS=mars-f67v8-egress \
  MCP_URL=http://host.containers.internal:7101/mcp \
  SESSION_EXTRA_HOSTS=host.containers.internal:host-gateway \
  DATA_DIR=<run>/data DATA_DIR_HOST=<run>/data \
  SECRETS_MASTER_KEYS="1=$(openssl rand -base64 32)" ... target/debug/mars-orchestrator

# The credential, stored as the global agent credential through the ordinary
# secrets endpoint (SPEC.md, "Secrets", Agent credentials). The value goes from
# the operator's file straight into the request body and nowhere else.
POST /api/secrets {"scope":"global","name":"CLAUDE_CODE_OAUTH_TOKEN","value":<from the file>}

# One project per scenario (POST /api/projects with the file:// remote), one
# task, one session on the project's own seeded `implementer`
# (POST /api/projects/{pid}/sessions {"profile_id":…, "task_id":…}).
```

**Why `--features integration-tests`.** `RemoteUrl::parse` accepts a `file://`
remote only under that feature (`orchestrator/src/models/project.rs`); a release
build takes `https://` only. A run against a release binary would have to point
the projects at a real remote, which this procedure deliberately does not do.
The feature also makes Argon2 cheap and adds `/api/test/users`; neither touches
what is being verified here, which is what happens inside the session container.

**The one profile edit.** The seeded `implementer` of each project was read
*before* anything was changed, and its `image` and `system_prompt` are what the
acceptance criterion is about; only `model` was then set to
`claude-haiku-4-5`, to keep the four runs cheap. Nothing else about any profile
was touched.

## Observed on the dev image, 2026-09-21

- [x] **Seeded with no manual edit.** With `SESSION_IMAGE_DEFAULT` pointing at
  `localhost/mars-session-claude-dev:latest`, all four profiles of every new
  project (`planner`, `implementer`, `reviewer`, `merger`) came up on that image,
  and every `implementer`'s `system_prompt` carried the environment paragraph
  verbatim ("Your session runs in a container of its own … There is no root
  here: no `apt`, no `sudo` …"). The four projects' prompts were byte-identical
  (one sha256). `model` was null, `serves_states` `["ready"]`, `is_default`
  true.
- [x] **A: toolchain present.** `cargo build` and `cargo test` ran in the
  container with no installation of any kind and no `cargo: command not found`
  anywhere in the transcript — the first `cargo build` compiled the crate in
  0.07 s. `npm test` ran on the image's Node 22.23.2. 29 turns, 0.1615 USD.
  (This scenario's fixture had two defects of its own: a `node --test js/`
  script this Node rejects, which the agent diagnosed and fixed, and a missing
  `.gitignore`, so `git add -A` swept `target/` into the commit. Both are the
  fixture's, not the image's; the other three repositories were repaired before
  their runs and their commits touch only source files.)
- [x] **B: pinned toolchain.** The repository pinned `channel = "1.90.0"`,
  which is not the image's 1.98.1. The first `rustc --version` printed
  `info: syncing channel updates for 1.90.0-x86_64-unknown-linux-gnu` … `warn:
  the missing active toolchain 1.90.0-x86_64-unknown-linux-gnu has been
  auto-installed`, then `rustc 1.90.0`. rustup installed it at runtime, as
  `agent`, with no root and no prompting, because `/opt/rustup` and
  `/opt/cargo` belong to uid 1000 (`ARCHITECTURE.md`, "Session image"). Build
  and tests then passed on the pinned toolchain and the agent reported the
  version in its task comment. 16 turns, 0.0819 USD.
- [x] **C: missing tool.** `cargo nextest run` failed with ``error: no such
  command: `nextest` ``. The agent installed it and carried on rather than
  reporting a blocker or handing off: it ran `cargo install cargo-nextest`
  first, then — after that compile outlasted the CLI's 120-second foreground
  Bash timeout — `cargo binstall cargo-nextest -y`, which resolved the GitHub
  release and put the binary in `/opt/cargo/bin` in 1.36 s. `cargo nextest run`
  then passed both tests. 31 turns, 0.1796 USD. The detour is a prompt-wording
  finding, not an image one; see below.
- [x] **D: root needed.** The repository demanded a `libpq-dev` install. The
  agent ran `sudo apt-get install -y libpq-dev` **once**, got
  `/bin/bash: line 1: sudo: command not found` (exit 127), and stopped trying:
  no retry, no `apt-get` without `sudo`, no loop. It committed the code, wrote a
  task comment naming the blocker ("cannot be installed in the container
  environment because `sudo` is not available") and escalated through
  `needs_human` with the same reason — which is what the paragraph asks for.
  15 turns, 0.0811 USD.
- [x] **The login shell resolves the toolchain.** In the running session
  container of scenario A, `/bin/bash -l -c …` as `1000:1000` printed
  `PATH=/opt/cargo/bin:/session/home/.npm-global/bin:/usr/local/bin:/usr/bin:…`
  and resolved `cargo` → `/opt/cargo/bin/cargo`, `rustc` →
  `/opt/cargo/bin/rustc`, `node` → `/usr/local/bin/node`, `npm` →
  `/usr/local/bin/npm`, with `cargo 1.98.1` and `node v22.23.2`. This is the
  `/etc/profile.d/mars-toolchains.sh` half of the `PATH` contract, over the
  real `/session/home` bind mount. It was proven with `podman exec` of that
  command into the session's own container rather than by driving the terminal
  WebSocket; the command and the user are the ones `ws/terminal.rs` uses
  (`TERMINAL_CMD = ["/bin/bash", "-l"]`), and the engine suite already covers
  the socket itself.
- [x] **Nothing installed reached a commit.** Each session branch was read back
  out of the project mirror after the session ended. B and C committed
  `src/lib.rs` alone; D committed `src/lib.rs` and `Cargo.lock`. No path under
  a tool's install location appears in any of them, which follows from where the
  toolchain lives: `/opt/cargo`, `/opt/rustup` and `/session/home/.npm-global`
  are all outside `/session/work`. A's `target/` is build output from a fixture
  without a `.gitignore`, not an installed tool.
- [x] **The credential never surfaced.** The orchestrator log and every event
  dump of all four sessions were checked against the operator's token file:
  `grep -c -F -f` printed `0` for each. No session produced a `launch_warning`,
  so the credential resolved and was injected without being declared on any
  profile (ADR 0036).

## Not covered by this run

`corepack`, `pnpm`/`yarn` and `npm install -g` into
`NPM_CONFIG_PREFIX=/session/home/.npm-global` were not exercised: no scenario
needed a global Node tool. The image's own build-time assertions cover that they
resolve (`images/claude-dev/Dockerfile`), and `images/smoke-test.sh` with
`DEV_IMAGE` set checks them over an empty home mount; what is untested live is an
agent choosing that route for a missing Node tool. A future run of this
procedure should add a fifth scenario for it.

## Finding: the paragraph does not point at `cargo binstall`

`cargo-binstall` is in the image precisely so that installing a cargo tool does
not cost minutes of compiling, but the environment paragraph offers ``cargo
install` or `cargo binstall`` as an undifferentiated pair, and in scenario C the
agent took the first one. Compiling `cargo-nextest` meant 465 crates, a
foreground Bash call pushed into the background by the CLI's own timeout, and
about a dozen turns spent polling a log before the agent gave up and reached for
`binstall`, which then finished in under two seconds. The image is right and the
behaviour is right; the wording costs money on every session that needs a cargo
tool. The paragraph now reaches for `cargo binstall` first and names `cargo
install` as the fallback (`vbtbj`, `SPEC.md`, "Role profile templates"), which
reaches new projects only, as every template change does.
