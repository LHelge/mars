# Claude translation fixtures

The regression suite that proves the Claude translator against every pinned CLI
version (ADR 0008; `CLAUDE.md`, "Testing expectations"; `SPEC.md`, "AgentEvent",
translation rules). The runner is `orchestrator/tests/agent_fixtures.rs`. It
needs no credentials, no network and no container engine, so CI runs it on every
change to `src/agent/`.

## Layout

One directory per pinned CLI version, named exactly as the version the image
pins (`src/agent/claude/mod.rs`, `CLAUDE_CLI_VERSION`):

```
tests/fixtures/claude/<version>/
  NOTES.md                      what the probe observed on this version
  <scenario>.jsonl              the native stream-json lines, one per line
  <scenario>.stdin.jsonl        optional: the lines the owner would have written
  <scenario>.expected.json      the state and the exact event sequence
```

`<scenario>.jsonl` is either a recording made by `tests/claude_probe.rs` against
the real CLI, or — for a rule the live CLI cannot be made to produce on
demand — a hand-written file whose name starts with `synthetic_`.

`<scenario>.expected.json` is:

```json
{
  "state": {
    "resumed": false,
    "partial_messages": false,
    "credential": { "name": "CLAUDE_CODE_OAUTH_TOKEN", "scope": "project" }
  },
  "events": [ /* AgentEvent, as SPEC.md "AgentEvent" spells it */ ]
}
```

`state` is the `TranslateConfig` the owner would have built for that launch;
`credential` is `null` when no model credential was injected, and its `name` is
`ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN` and its `scope` `global`,
`user` or `project`. `events` is compared as JSON against the translator's
output serialised through `AgentEvent`, so a field name that drifts from the
spec fails the suite.

The runner seeds the echo-suppression hashes from every `user` line of
`<scenario>.stdin.jsonl`, using the message's text exactly as
`record_sent_input` stores it; a `control_request` line seeds nothing.

## The rules

- **A recording is never edited.** A CLI version bump adds a directory; the old
  one keeps proving what that version did. Fixing an expected file is only ever
  allowed for a translator bug the old file encoded, and never by rewriting the
  recorded `.jsonl`.
- **Every `<scenario>.jsonl` needs a `<scenario>.expected.json`.** One without
  fails the runner with `missing expected events for <version>/<scenario>`, so a
  recording cannot sit in the tree proving nothing.
- **Expected events are written by hand**, read line by line off the recording
  against `SPEC.md`. `MARS_WRITE_EXPECTED=1 cargo test --test agent_fixtures`
  rewrites every expected file with what the translator currently answers and
  then fails the run; that output is a starting point to review, never an
  expectation. A generated file committed unreviewed proves only that the
  translator agrees with itself.
- **Recordings carry no credentials.** The probe scrubs before writing, and
  `no_fixture_carries_a_credential` greps every file for an `sk-ant-` key other
  than the fake probe one and for a `Bearer ` header other than
  `Bearer <token>`.
- Host-specific values — temp paths, session ids, message ids, timestamps,
  costs, token counts — are copied into the expected file verbatim. These are
  recordings, not templates.

## Large values

A fixture never commits hundreds of kilobytes of filler. Any JSON object with a
`__generate` key, in a `.jsonl` line or in an expected event, is replaced by a
string of `n` ASCII bytes before the comparison:

```json
{ "__generate": "tool_result_bytes", "n": 307200 }
```

`fill` picks the byte (one ASCII character, `x` by default). `synthetic_truncation`
uses it on both sides: a 300 KiB tool result in the native line and the exact
256 KiB `TOOL_RESULT_MAX_BYTES` cut in the expected `tool_result`.

## Coverage

`every_translator_event_kind_is_covered_by_the_pinned_version` fails unless every
`AgentEventBody` kind the Claude translator can emit — `init`, `text_delta`,
`text`, `thinking`, `tool_call`, `tool_result`, `permission_denied`,
`subagent_start`, `subagent_end`, `result`, `error`, `raw` — appears in at least
one expected file of the pinned version, so a rule cannot go silently uncovered.
`the_subagent_recording_proves_the_subagent_tool_name` fails if the recording
stops producing `subagent_start`/`subagent_end`, which is how
`SUBAGENT_TOOL_NAMES` is proven per version (`ARCHITECTURE.md`, "Claude Code
invocation").

The synthetic scenarios cover what a live recording cannot produce on demand:

| scenario | rule |
| --- | --- |
| `synthetic_truncation` | a tool result over `TOOL_RESULT_MAX_BYTES` is cut and marked `truncated`; one exactly at the limit is not |
| `synthetic_echo` | the echo of a message the owner wrote is dropped once; a second identical line and a line nobody wrote are `raw`; `[Request interrupted by user]` is dropped |
| `synthetic_denials` | a `system`/`permission_denied` and the same `tool_use_id` in `result.permission_denials` produce one event; an id only the result reports produces its own |
| `synthetic_raw` | a non-JSON line, a JSON array, an unknown top-level type, an unknown `system` subtype and an unknown content block are all kept as `raw` |
| `synthetic_partial` | `content_block_delta`/`text_delta` becomes `text_delta`; every other stream event is dropped |
| `synthetic_thinking` | a thinking block with text is an event, an empty one is nothing, `redacted_thinking` is `redacted: true` |
| `synthetic_init_resumed` | `state.resumed` sets `init.resumed`; a repeated `session_id` produces nothing and a new one a second `init` |
| `synthetic_auth_no_credential` | an authentication failure with no injected credential names both variables and is emitted once |
