#!/usr/bin/env bash
#
# Record the MCP conformance fixtures of the pinned Claude Code CLI
# (images/claude/VERIFY.md, "Recording the MCP conformance fixtures").
#
# Runs the pinned session image, with the flags a conversational launch uses,
# against the real MCP router behind a recorder (the `record` scenario of
# orchestrator/tests/mcp_conformance.rs), and writes one fixture per request to
# orchestrator/tests/fixtures/mcp/<version>/. The model is fake_anthropic.py
# beside this script, so no model credential is needed and the tool calls are
# the same on every run; the only token involved is the seeded session's, which
# lives in a temporary directory and is never written to a fixture.
#
# Environment:
#   ENGINE        podman (default) or docker
#   DOCKER_HOST   the engine socket the test suite's Postgres runs on
#                 (default: the rootless Podman socket)
#   IMAGE         the session image (default mars-session-claude:<pinned version>)
#   FAKE_PORT     port of the fake Messages API on the host (default 7099)
#   KEEP          set to keep the scratch directory (logs, CLI home) for debugging
#
# The MCP listener binds port 7001 on the host: the container reaches it as
# `orchestrator:7001`, the default MCP_URL, so the recorded Host is the one a
# compose deployment's sessions send.

set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
ENGINE=${ENGINE:-podman}
V=$(sed -n 's/^ARG CLAUDE_CODE_VERSION=//p' "$ROOT/images/claude/Dockerfile")
IMAGE=${IMAGE:-localhost/mars-session-claude:$V}
FAKE_PORT=${FAKE_PORT:-7099}
OUT="$ROOT/orchestrator/tests/fixtures/mcp/$V"
export DOCKER_HOST=${DOCKER_HOST:-unix:///run/user/$(id -u)/podman/podman.sock}
if [ "$ENGINE" = podman ]; then GATEWAY=host.containers.internal; else GATEWAY=host.docker.internal; fi

[ -e "$OUT" ] && { echo "$OUT exists; a recorded version is never overwritten" >&2; exit 1; }

S=$(mktemp -d)
mkdir -p "$S"/{work,home,log,config}
git -C "$S/work" init -q && echo "# fixture" > "$S/work/README.md"
pids=()
cleanup() {
  "$ENGINE" rm -f mars-mcp-record >/dev/null 2>&1 || true
  # The recorder is cargo's child, not ours: `abort` is how it is told to stop
  # without writing anything.
  [ -e "$S/done" ] || { touch "$S/abort"; wait "${recorder:-}" 2>/dev/null || true; }
  for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done
  if [ -n "${KEEP:-}" ]; then echo "kept $S" >&2; else rm -rf "$S"; fi
}
trap cleanup EXIT

python3 "$ROOT/scripts/mcp-record/fake_anthropic.py" "$FAKE_PORT" 2>"$S/fake.log" &
pids+=($!)

(cd "$ROOT/orchestrator" && SQLX_OFFLINE=true MARS_MCP_RECORD="$OUT" MARS_MCP_RECORD_CONFIG="$S/mcp.json" \
  cargo test --features integration-tests --test mcp_conformance record -- --exact --nocapture) \
  >"$S/recorder.log" 2>&1 &
recorder=$!
pids+=($recorder)
until [ -s "$S/mcp.json" ]; do
  kill -0 "$recorder" 2>/dev/null || { cat "$S/recorder.log" >&2; exit 1; }
  sleep 1
done

# The flags of a conversational launch (ARCHITECTURE.md, "Claude Code
# invocation"), without a model or a system prompt. The fake key is not a
# credential; it only lets the CLI start a turn against the fake API.
"$ENGINE" run -d --name mars-mcp-record --user 1000:1000 --cap-drop ALL \
  --security-opt no-new-privileges \
  $([ "$ENGINE" = podman ] && echo --userns=keep-id:uid=1000,gid=1000) \
  --add-host "orchestrator:host-gateway" --add-host "$GATEWAY:host-gateway" \
  -v "$S/work:/session/work" -v "$S/home:/session/home" -v "$S/log:/session/log" \
  -v "$S/config:/session/config" -v "$S/mcp.json:/session/mcp.json:ro" \
  -e HOME=/session/home -e CLAUDE_CONFIG_DIR=/session/config \
  -e ANTHROPIC_BASE_URL="http://$GATEWAY:$FAKE_PORT" -e ANTHROPIC_API_KEY=fake-not-a-key \
  -e CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 \
  "$IMAGE" claude --print --output-format stream-json --input-format stream-json --verbose \
  --forward-subagent-text --system-prompt-snapshot off --include-partial-messages \
  --permission-mode bypassPermissions --permission-prompts none --mcp-config /session/mcp.json \
  >/dev/null

# One turn: the fake model calls the tools of its script, then answers.
echo '{"type":"user","message":{"role":"user","content":[{"type":"text","text":"Call the tools."}]}}' |
  "$ENGINE" exec -i mars-mcp-record sh -c 'exec cat >/tmp/mars-stdin'
# The entrypoint appends the CLI's stdout to /session/log/stream.jsonl.
for _ in $(seq 120); do
  grep -qs '"type":"result"' "$S/log/stream.jsonl" && break
  sleep 1
done
grep -qs '"type":"result"' "$S/log/stream.jsonl" ||
  { echo "no result line" >&2; tail -20 "$S/log/stream.jsonl" "$S/log/stderr.log" >&2; exit 1; }

# Stop it the way the session owner does, and give the CLI time to close its
# MCP connection before the recorder stops listening.
"$ENGINE" kill --signal=SIGINT mars-mcp-record >/dev/null
"$ENGINE" wait mars-mcp-record >/dev/null || true
sleep 2
touch "$S/done"
wait "$recorder" || { cat "$S/recorder.log" >&2; exit 1; }
grep '^recorder:' "$S/recorder.log"

# The CLI's own view of the MCP server, printed for NOTES.md and never kept
# beside the fixtures.
echo "--- the CLI's MCP log"
cat "$S"/home/.cache/claude-cli-nodejs/*/mcp-logs-mars-orchestrator/*.jsonl 2>/dev/null |
  sed 's/Bearer [A-Za-z0-9._~+\/=-]*/Bearer <scrubbed>/g' || true
echo "---"

if grep -rEl 'Bearer|sk-ant[-]|[Aa]uthorization' "$OUT"; then
  echo "a fixture carries a credential-shaped value; not keeping it" >&2
  rm -rf "$OUT"
  exit 1
fi
echo "recorded $(ls "$OUT"/*.json | wc -l) requests into $OUT"
