Every session starts in a fresh container with a fresh checkout, so by default each one downloads its dependencies and builds from scratch. A shared directory is a directory Mars keeps once for the whole project and mounts read-write into every session of it, at a path you choose. Ten sessions then share one build cache instead of filling ten. Shared directories belong to one project and are managed on its **Shared directories** tab.

### What is safe to share

A directory is safe to share when several sessions can write to it at the same time without breaking each other. A download cache almost always is, because what it holds is looked up by content and never rewritten. Build output inside the checkout usually isn't, because each session's branch rewrites it in place. Cargo is the exception: it locks its build directory, so builds from several sessions wait for each other instead of corrupting it. Every session's checkout is at the same path, `/session/work`, so what one session built is reused by the next.

Some starting points follow. The tab offers each named entry as a preset that fills in the form:

| Ecosystem | Share (name → container path) | Keep per session |
| --- | --- | --- |
| Rust | `target` → `/session/work/target`; `cargo-registry` → `/opt/cargo/registry` | |
| Node | `npm-cache` → `/session/home/.npm`, or the pnpm store | `node_modules` |
| Go | `go-mod` → `/session/home/go/pkg/mod`; `go-build` → `/session/home/.cache/go-build` | |
| Python | `uv-cache` → `/session/home/.cache/uv`, or the pip cache | virtualenvs |
| JVM | `m2` → `/session/home/.m2`; `gradle` → `/session/home/.gradle` | `build/` |

`node_modules` and virtualenvs stay per session because they are rewritten in place on every install, and two branches seldom agree on a lockfile. Share the package cache behind them instead. That already saves the downloads.

Cargo's registry lives under `$CARGO_HOME/registry`, so its path depends on the image. The `mars-session-claude-dev` image sets `CARGO_HOME=/opt/cargo`, which gives `/opt/cargo/registry`. An image that keeps Cargo's default home uses `/session/home/.cargo/registry` instead.

### Choosing a path

The **name** is the directory's name on disk: up to 64 lowercase letters, digits, `-` and `_`, starting with a letter or digit. The **container path** is where every session mounts it:

- an absolute path, written plainly, with no `.`, `..`, doubled or trailing slashes and no spaces;
- not `/data` or anything below it;
- not `/session/work`, `/session/home`, `/session/log` or `/session/mcp.json`, and not a parent of one of them, because the directory would hide what the session needs there.

A path *inside* `/session/work`, `/session/home` or `/session/log` is fine. `/session/work/target` is the usual example. A directory inside the checkout should be one your repository already ignores, as `target` is in a Rust project. Each name and each path can be used once per project.

### When changes take effect

The list is read when a session launches. A session that is already running keeps the mounts it started with, and it picks up a new or removed directory at its next launch.

### Emptying and removing

A shared directory grows across branches and never shrinks by itself. When disk gets tight, **Clear** it: the directory stays, empty, and the next build fills it again. **Remove** deletes the directory and its contents, and later sessions no longer mount it. Both are refused while any session of the project is creating or running, because a session could be using it. The tab disables both buttons while that is so. Parked sessions don't count.

Deleting the project deletes its shared directories with it.
