# 0033. No interactive prompts: the agent never asks the host a question

Status: accepted.

## Context

`SPEC.md` carried a `prompt` event kind (`prompt_id`, `text`, optional `options`) and a matching `answer` input carrying `reply_to: <seq>`, on the assumption that a coding agent might ask the user a question mid-turn — through the CLI's `AskUserQuestion` tool, or through a permission request — and that the host would have to route the question to the browser and the answer back to stdin. The rule that went with it said an `answer` whose prompt had already been consumed was rejected with `input_rejected`, and the frontend kept a `pendingPrompt` in the session store so the composer could switch to answer mode.

Mars launches the CLI with `--permission-mode bypassPermissions --permission-prompts none`, so nothing was expected to ask. Whether anything could was one of the adapter's verification items, and the live probe against the pinned version (2.1.274) answered it. Asked to use `AskUserQuestion`, the CLI did not offer the tool at all: the model searched for it, found nothing, and asked its question as ordinary assistant text. The turn ended with a `result` of subtype `success` and `is_error: false`, the process exited 0 on stdin EOF, and nothing ever blocked on stdin (`orchestrator/tests/fixtures/claude/2.1.274/NOTES.md`, `prompt_kind`, and the recording beside it). No permission request reached the host either: a denial arrives as a `permission_denied` system message that reports a decision already taken, never as a question.

## Decision

There is no `prompt` event kind and no `answer` input in v1. `SessionInput` has one kind, `message`; `user_message` has no `reply_to`; `AgentEvent` has no `prompt`; the session store has no `pendingPrompt` and the composer has no answer mode. A question the model writes as text is ordinary assistant text, and the user answers it with an ordinary `message`, which the CLI queues as the next turn (`ARCHITECTURE.md`, "Input encoding").

The rejected alternative was to keep the two shapes as dormant contract — defined in the spec and the types, never emitted — so that a backend that does ask would need no schema change. It was rejected because a kind nothing can produce is a kind nothing is tested against: the translator had no rule that emitted it, the frontend would carry answer-mode UI that could not be exercised, and `reply_to` bookkeeping would have to be maintained through the owner's single-writer input path for a case that cannot arise. The cost of adding it back is one event kind, one input kind and their translation rule, all of which a second backend would have to define anyway, because the native shape of its question is not knowable from here.

## Consequences

- The input path has one shape. `encode_input` writes the same SDK user-message line for every input, and the owner's `input_rejected` reasons are about deliverability (ephemeral session, wrong state, unknown kind) rather than about a consumed prompt.
- Anything the model says that reads like a question is just text. The UI does not distinguish it, and the user is never blocked from sending an ordinary message.
- The finding is version-specific: it holds for the pinned CLI version, with those permission flags, and the probe re-checks it on every version bump. A version that did route a question to the host would fail the probe's `prompt_kind` scenario, and reintroducing the kind is then a spec change with a fixture of its own.
- A second `AgentBackend` that genuinely prompts adds the kind back with its own translation rule; nothing in the trait or the event envelope prevents it.
