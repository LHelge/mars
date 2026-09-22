A profile is the recipe for a project's sessions. It sets how a session runs, which work it picks up, what it runs in and what it may touch. Every session is launched from one. Profiles belong to one project and are edited on its **Profiles** tab. A change applies to the next launch and leaves running sessions alone.

Two fields decide an agent's role: its **served states** and its **system prompt**. A planner serves `backlog` and hands tasks on to `ready`. A reviewer serves `review` and sends each task either on to be merged or back to `ready`.

### Kind: conversational or ephemeral

| | Conversational | Ephemeral |
| --- | --- | --- |
| Input | A conversation. You send messages, even mid-turn. | One prompt: the task and your message. There is no composer. |
| When it goes quiet | Parks. Your next message resumes it where it stopped, and any task it holds stays held. | Stopped and failed as stalled. Its task is released. |
| After a failure | Can be retried. | Never retried. Launch a new run, which can start from the finished run's branch. |
| Automation | Only by hand. | Can be launched by the dispatcher or on a schedule ([Automation](help:automation)). |

The kind also sets the default for **partial messages**: on for conversational, so text appears as it is written, and off for ephemeral.

### Served states

These are the task states a profile picks work up from. Only queue states can be served. The agent's `ready` tool lists the unheld, unblocked tasks in them, and the agent can claim only those. When you launch a session for a named task, the task's state doesn't matter. [Task flow](help:task-flow) covers states, claims and hand-offs.

### Model

The model is passed to the agent CLI as it is. Use a model alias the CLI knows or a full model id. Leave it empty to use the CLI's default.

### Image

This is the container image a session runs in. It is pulled when a launch needs it, and a pull that fails fails the launch. Two images come with Mars:

- **`mars-session-claude`** is the base: the agent CLI, `git`, Node and a modest toolchain.
- **`mars-session-claude-dev`** is built on the base and adds Rust, pnpm and yarn, and the system libraries builds commonly need. It is the usual default.

Sessions have no root, so nothing can install system packages while a session runs. Agents can still install user-level tools, such as `rustup` toolchains, `cargo install`, `npm install -g` or a Python virtual environment. If your project needs a system library neither image has, build an image `FROM` a Mars session image and name it here. Building and publishing that image is done on the host.

### Runtime

A sandboxed container runtime, such as `runsc` (gVisor) or `kata`. Leave it empty for the engine's default. The runtime must be installed and registered with the container engine on the host, and a runtime the engine doesn't know makes every launch fail.

### Idle timeout

This is how many seconds a running session may go without a new event, 1800 by default. When the time runs out, a conversational session parks and an ephemeral one is stopped as stalled. Silence is what counts, so a long build or test run that prints nothing is idle too. Raise the timeout for a profile whose work includes long quiet commands.

### Git tools

The task tools are always available. Four further tools act on the project's git mirror, and a profile grants each one separately:

| Tool | What it lets the agent do |
| --- | --- |
| `list_session_branches` | List session branches and how far each is ahead of or behind the default branch. |
| `merge` | Merge into an integration branch. A task's work merges only as its approved hand-off, and conflicts are never resolved for it. |
| `rebase` | Rebase one branch onto another in the mirror, such as a session branch onto the default branch. |
| `push` | Publish an integration or session branch to the remote, using the project's [git credential](help:git-credential). |

`push` is the one that reaches outside Mars, and the starter profiles don't grant it. Grant it only to a profile whose job is to publish. An implementer doesn't need `rebase` either, because it can fetch and rebase inside its own clone. [Branches and merging](help:branches) explains the branches these tools act on.

### Secrets

These are the names of any [secrets](help:secrets) the agent's job needs as environment variables, such as a package registry token. The [agent credential](help:agent-credentials) is never listed here, because every session receives it anyway. The profile editor shows which credential a launch would use.

### System prompt

The system prompt is added to the agent CLI's own system prompt on every launch. Use it to describe the agent's job: its role, which tools to use and how to hand work on. How work is done in this repository comes from the repository itself: its `CLAUDE.md`, its `.mcp.json` and its [skills](help:skills). Keep conventions, test commands and style rules there, where every profile picks them up.

A starter profile's prompt is your project's own copy, so editing it changes no other project. The starter prompts name the project's states, such as `ready` and `review`, by name. If you rename one of those states, the profile still serves it, but edit the prompt to use the new name.

### Default profile

Each project has exactly one default profile, marked `default` on the **Profiles** tab. It is the one already selected in the project's launch form, and a new profile starts on its image. The default profile can't be deleted, and neither can a profile that still has sessions.
