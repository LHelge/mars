#!/usr/bin/env bash
# smoke-test.sh — run the built Mars session images through a real container
# engine and prove the session image contract (ARCHITECTURE.md, "Session
# image", "Stop semantics", "Session container specification", "Uid contract";
# ADR 0004, ADR 0010). Used locally and by the Images CI workflow (README.md,
# "CI": "Build session images; smoke-run the entrypoint").
#
# It proves, on whichever engine ENGINE names:
#   - the entrypoint redirects stdout to /session/log/stream.jsonl and stderr
#     to /session/log/stderr.log, appends to both, and leaves the container's
#     own stdout empty (ADR 0010);
#   - the process is uid 1000 with HOME=/session/home and cwd /session/work
#     (ARCHITECTURE.md, "Uid contract");
#   - a session without a command fails loudly with exit 2;
#   - the claude image carries the pinned CLI, git and a working login bash;
#   - the stub replays its fixture under both `-p` and `--input-format
#     stream-json`, opens every turn with a `system`/`init` line as the real
#     CLI does, writes nothing at all before the first stdin line in the
#     interactive mode (ADR 0032), and reports the MCP server from
#     --mcp-config as connected;
#   - the CLI reads the FIFO /tmp/mars-stdin its entrypoint made, and a writer
#     of that FIFO going away is not an EOF for it (ADR 0034);
#   - the agent CLI is PID 1, SIGINT ends the turn with the real CLI's
#     interrupt shape (a `user` line `[Request interrupted by user]` and a
#     `result` with `subtype: "error_during_execution"`, `is_error: true`,
#     `terminal_reason: "aborted_streaming"`) and exit 0,
#     and SIGTERM exits 143 (ARCHITECTURE.md, "Stop semantics"). A signal that
#     failed to reach PID 1 would mean the launcher needs `Init: true`, which
#     is the engine adapter's decision, not this script's to work around.
#
# Real-CLI SIGINT behaviour is deliberately not exercised here: it needs model
# credentials. The claude image is only run through `claude --version`,
# `git --version` and `bash -l`; the credentialed verification task of the
# session-images epic covers SIGINT against the real CLI.
#
# Environment:
#   ENGINE          docker | podman        (default docker)
#   CLAUDE_IMAGE    claude image reference (default mars-session-claude:dev)
#   STUB_IMAGE      stub image reference   (default mars-session-stub:dev)
#   SMOKE_KEEP_DIR  when set, temp directories are created under this path and
#                   kept, so CI can upload log/* after a failure.
#
# Nothing this script prints carries an argument list, an environment or the
# contents of the smoke mcp.json (CLAUDE.md, rule 3). That mcp.json holds the
# literal, obviously fake bearer token `smoke-test-token`.

set -euo pipefail

ENGINE="${ENGINE:-docker}"
CLAUDE_IMAGE="${CLAUDE_IMAGE:-mars-session-claude:dev}"
STUB_IMAGE="${STUB_IMAGE:-mars-session-stub:dev}"

SCRIPT_DIR="$(dirname "$(readlink -f "$0")")"
CLAUDE_DOCKERFILE="$SCRIPT_DIR/claude/Dockerfile"
FIXTURE="$SCRIPT_DIR/stub/fixtures/default.jsonl"

# The SDK user-message shape the orchestrator writes to stdin
# (ARCHITECTURE.md, "Input encoding").
USER_LINE='{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hello"}]}}'

# The conversational argv of ARCHITECTURE.md, "Claude Code invocation", without
# the optional --model, --append-system-prompt, --resume and
# --include-partial-messages.
CONVERSATIONAL_ARGV=(
    claude --print
    --output-format stream-json --input-format stream-json --verbose
    --forward-subagent-text --system-prompt-snapshot off
    --permission-mode bypassPermissions --permission-prompts none
    --mcp-config /session/mcp.json
)

TMP=""
CONTAINERS=()
BG_PIDS=()
RUN_FLAGS=()
BG_NAME=""
BG_PID=""
LAST_CONTAINER=""
CHECKS=0
FAILURES=0

usage() {
    cat <<'USAGE'
Usage: images/smoke-test.sh [--help]

Runs the built Mars session images through a real container engine and checks
the session image contract. Every check prints "ok: <name>" or "FAIL: <name>";
the script exits non-zero if any check failed.

Environment:
  ENGINE          docker | podman                (default: docker)
  CLAUDE_IMAGE    claude session image reference (default: mars-session-claude:dev)
  STUB_IMAGE      stub session image reference   (default: mars-session-stub:dev)
  SMOKE_KEEP_DIR  create temp directories under this path and keep them

Examples:
  images/smoke-test.sh
  ENGINE=podman images/smoke-test.sh
  ENGINE=podman SMOKE_KEEP_DIR=/tmp/smoke images/smoke-test.sh
USAGE
}

cleanup() {
    local status=$?
    local pid name
    for pid in ${BG_PIDS[@]+"${BG_PIDS[@]}"}; do
        kill "$pid" 2>/dev/null || true
    done
    for name in ${CONTAINERS[@]+"${CONTAINERS[@]}"}; do
        "$ENGINE" rm -f "$name" >/dev/null 2>&1 || true
    done
    for pid in ${BG_PIDS[@]+"${BG_PIDS[@]}"}; do
        wait "$pid" 2>/dev/null || true
    done
    if [ -n "$TMP" ]; then
        if [ -n "${SMOKE_KEEP_DIR:-}" ]; then
            printf 'kept: %s\n' "$TMP"
        else
            rm -rf "$TMP"
        fi
    fi
    return "$status"
}

# --- small helpers ----------------------------------------------------------

# A random suffix so a container left behind by an aborted run never collides
# with this one's names.
container_name() {
    printf 'smoke-%s-%s\n' "$1" "$(tr -dc 'a-z0-9' </dev/urandom | head -c 8)"
}

# session_dir <name> — a fresh <tmp>/<name>/{work,home,log} triple.
# Under Docker the directories are world-writable so uid 1000 inside the
# container can write them on a runner whose own uid is not 1000; rootless
# Podman maps the runner's uid onto 1000 with keep-id instead.
session_dir() {
    local dir="$TMP/$1"
    mkdir -p "$dir/work" "$dir/home" "$dir/log"
    if [ "$ENGINE" != podman ]; then
        chmod 0777 "$dir" "$dir/work" "$dir/home" "$dir/log"
    fi
    printf '%s\n' "$dir"
}

# write_mcp_config <dir> — the smoke mcp.json, bind-mounted read-only at
# /session/mcp.json. Its bearer token is the literal `smoke-test-token`;
# neither the file nor its contents are ever printed.
write_mcp_config() {
    printf '%s\n' '{"mcpServers":{"mars-orchestrator":{"type":"http","url":"http://host.containers.internal:7001/mcp","headers":{"Authorization":"Bearer smoke-test-token"}}}}' >"$1/mcp.json"
    chmod 0644 "$1/mcp.json"
}

# session_flags <dir> — the session-equivalent host config of ARCHITECTURE.md,
# "Session container specification", into the RUN_FLAGS global.
session_flags() {
    local dir=$1
    RUN_FLAGS=(
        --user 1000:1000
        --workdir /session/work
        -e HOME=/session/home
        --cap-drop ALL
        --security-opt no-new-privileges
        -v "$dir/work:/session/work"
        -v "$dir/home:/session/home"
        -v "$dir/log:/session/log"
    )
    if [ "$ENGINE" = podman ]; then
        RUN_FLAGS+=("--userns=keep-id:uid=1000,gid=1000")
    fi
    if [ -f "$dir/mcp.json" ]; then
        RUN_FLAGS+=(-v "$dir/mcp.json:/session/mcp.json:ro")
    fi
}

# run_once <image> <dir> <cmd...> — one throwaway container with the session
# host config. Returns the container's exit code and leaves its name in
# LAST_CONTAINER so the caller can read `$ENGINE logs`; the container itself is
# removed by the EXIT trap, because `--rm` would take `logs` with it.
run_once() {
    local image=$1 dir=$2
    shift 2
    local name rc=0
    name="$(container_name run)"
    CONTAINERS+=("$name")
    LAST_CONTAINER="$name"
    session_flags "$dir"
    "$ENGINE" run --name "$name" "${RUN_FLAGS[@]}" "$image" "$@" \
        >>"$dir/engine.out" 2>&1 || rc=$?
    return "$rc"
}

# start_interactive_stub <dir> <delay_ms> <cmd...> — the stub in the
# background, reading the FIFO its entrypoint made (ADR 0034). The container's
# own stdin is not opened, as the launcher leaves it. Sets BG_NAME and BG_PID.
start_interactive_stub() {
    local dir=$1 delay=$2
    shift 2
    local name
    name="$(container_name stub)"
    CONTAINERS+=("$name")
    session_flags "$dir"
    "$ENGINE" run --name "$name" "${RUN_FLAGS[@]}" \
        -e "MARS_STUB_LINE_DELAY_MS=$delay" \
        "$STUB_IMAGE" "$@" >>"$dir/engine.out" 2>&1 &
    BG_PID=$!
    BG_PIDS+=("$BG_PID")
    BG_NAME="$name"
}

# The relay the engine adapter's `attach_stdin` runs, one per line here: an
# exec that waits for the FIFO and `cat`s into it. Each relay ends when its
# line is written, which is what an orchestrator restart does to the owner's,
# and the CLI must not take that for an EOF (ADR 0034).
send_user_line() {
    local deadline
    # The container is started in the background and may not exist yet.
    deadline=$(($(date +%s) + 30))
    until [ "$("$ENGINE" inspect --format '{{.State.Running}}' "$BG_NAME" 2>/dev/null)" = true ]; do
        [ "$(date +%s)" -lt "$deadline" ] \
            || { fail "the container was not running within 30s"; return 1; }
        sleep 0.1
    done
    # shellcheck disable=SC2016 # the script is for the container's shell
    printf '%s\n' "$USER_LINE" | timeout 30 "$ENGINE" exec -i "$BG_NAME" sh -c \
        'i=0; while [ ! -p /tmp/mars-stdin ]; do i=$((i+1)); [ "$i" -le 100 ] || exit 1; sleep 0.1; done; exec cat >/tmp/mars-stdin' \
        || { fail "the user line could not be written to /tmp/mars-stdin"; return 1; }
}

# wait_for_line <file> <pattern> <timeout-secs> [count]
wait_for_line() {
    local file=$1 pattern=$2 timeout=$3 count=${4:-1}
    local deadline seen
    deadline=$(($(date +%s) + timeout))
    while :; do
        seen=0
        if [ -f "$file" ]; then
            seen="$(grep -c -F -- "$pattern" "$file" 2>/dev/null || true)"
        fi
        [ "${seen:-0}" -ge "$count" ] && return 0
        if [ "$(date +%s)" -ge "$deadline" ]; then
            printf 'timed out after %ss waiting for %s x %s in %s (saw %s)\n' \
                "$timeout" "$count" "$pattern" "$file" "${seen:-0}"
            return 1
        fi
        sleep 0.1
    done
}

# wait_for_lines <file> <count> <timeout-secs>
wait_for_lines() {
    local file=$1 count=$2 timeout=$3
    local deadline seen
    deadline=$(($(date +%s) + timeout))
    while :; do
        seen=0
        [ -f "$file" ] && seen="$(wc -l <"$file")"
        [ "${seen:-0}" -ge "$count" ] && return 0
        if [ "$(date +%s)" -ge "$deadline" ]; then
            printf 'timed out after %ss waiting for %s lines in %s (saw %s)\n' \
                "$timeout" "$count" "$file" "${seen:-0}"
            return 1
        fi
        sleep 0.1
    done
}

result_count() {
    local n
    n="$(grep -c -F '"type":"result"' "$1" 2>/dev/null || true)"
    printf '%s\n' "${n:-0}"
}

# One `system`/`init` line opens every turn, as the real CLI writes them
# (ARCHITECTURE.md, "Session image"; images/claude/VERIFY.md).
init_count() {
    local n
    n="$(grep -c -F '"subtype":"init"' "$1" 2>/dev/null || true)"
    printf '%s\n' "${n:-0}"
}

fail() {
    printf '%s\n' "$*"
    return 1
}

expect_eq() { # <actual> <expected> <what>
    [ "$1" = "$2" ] || fail "$3: expected '$2', got '$1'"
}

expect_contains() { # <file> <pattern> <what>
    grep -q -F -- "$2" "$1" || fail "$3: '$2' not found in $1"
}

expect_line() { # <file> <line> <what>
    grep -q -x -F -- "$2" "$1" || fail "$3: no line '$2' in $1"
}

# expect_silent <file> <secs> <what> — the file is still empty (or absent)
# after the given wait. Used where the contract is that nothing is written.
expect_silent() {
    local file=$1 secs=$2 what=$3 seen=0
    sleep "$secs"
    if [ -f "$file" ]; then
        seen="$(wc -l <"$file")"
    fi
    [ "${seen:-0}" -eq 0 ] || fail "$what: expected no lines, got ${seen} in $file"
}

# check <name> <cmd...> — run one check, time it, report it.
check() {
    local name=$1
    shift
    local start end rc=0
    CHECKS=$((CHECKS + 1))
    start=$(date +%s%N)
    "$@" >"$TMP/check.out" 2>&1 || rc=$?
    end=$(date +%s%N)
    if [ "$rc" -eq 0 ]; then
        printf 'ok: %s (%s ms)\n' "$name" "$(((end - start) / 1000000))"
    else
        FAILURES=$((FAILURES + 1))
        printf 'FAIL: %s (%s ms)\n' "$name" "$(((end - start) / 1000000))"
        sed 's/^/    /' "$TMP/check.out"
    fi
}

# --- checks -----------------------------------------------------------------

# The probe command of the redirect and append checks: one JSON line on stdout,
# one line on stderr, then the uid, HOME and cwd the contract fixes. $HOME is
# expanded by the shell inside the container, not by this one.
# shellcheck disable=SC2016
PROBE_SH='echo "{\"type\":\"smoke\"}"; echo oops >&2; id -u; echo "$HOME"; pwd'
PROBE_EXPECTED='{"type":"smoke"}
1000
/session/home
/session/work'

check_entrypoint_redirect() { # <image> <dir>
    local image=$1 dir=$2 logs
    run_once "$image" "$dir" sh -c "$PROBE_SH" \
        || { fail "the container exited non-zero"; return 1; }
    expect_eq "$(head -n 4 "$dir/log/stream.jsonl")" "$PROBE_EXPECTED" "stream.jsonl" || return 1
    expect_contains "$dir/log/stderr.log" oops "stderr.log" || return 1
    logs="$("$ENGINE" logs "$LAST_CONTAINER" 2>&1 | tr -d '[:space:]')"
    expect_eq "$logs" "" "the container's own stdout and stderr" || return 1
}

check_entrypoint_append() { # <dir>, already used by a redirect check
    local dir=$1
    run_once "$STUB_IMAGE" "$dir" sh -c "$PROBE_SH" \
        || { fail "the container exited non-zero"; return 1; }
    expect_eq "$(grep -c -F '{"type":"smoke"}' "$dir/log/stream.jsonl")" 2 \
        "JSON lines in stream.jsonl after the second run" || return 1
    expect_eq "$(wc -l <"$dir/log/stream.jsonl")" 8 \
        "total lines in stream.jsonl after the second run" || return 1
    expect_eq "$(grep -c -F oops "$dir/log/stderr.log")" 2 \
        "lines in stderr.log after the second run" || return 1
}

# The entrypoint's "no command given" message is written before any redirect is
# in place, so it lands on the container's own stderr and never in
# /session/log/stderr.log. Asserted through `$ENGINE logs`.
check_entrypoint_noargs() {
    local dir rc=0
    dir="$(session_dir noargs)"
    run_once "$STUB_IMAGE" "$dir" || rc=$?
    expect_eq "$rc" 2 "exit code of a container without a command" || return 1
    "$ENGINE" logs "$LAST_CONTAINER" >"$dir/logs.txt" 2>&1
    expect_contains "$dir/logs.txt" "no command given" "the container's own stderr" || return 1
    [ ! -s "$dir/log/stream.jsonl" ] || { fail "stream.jsonl is not empty"; return 1; }
}

check_claude_version() {
    local dir version
    dir="$(session_dir claude-version)"
    version="$(sed -n 's/^ARG CLAUDE_CODE_VERSION=//p' "$CLAUDE_DOCKERFILE")"
    [ -n "$version" ] || { fail "no ARG CLAUDE_CODE_VERSION in $CLAUDE_DOCKERFILE"; return 1; }
    run_once "$CLAUDE_IMAGE" "$dir" claude --version \
        || { fail "claude --version exited non-zero"; return 1; }
    run_once "$CLAUDE_IMAGE" "$dir" git --version \
        || { fail "git --version exited non-zero"; return 1; }
    run_once "$CLAUDE_IMAGE" "$dir" bash -lc 'echo ok' \
        || { fail "bash -lc exited non-zero"; return 1; }
    expect_contains "$dir/log/stream.jsonl" "$version" "the pinned CLI version" || return 1
    expect_contains "$dir/log/stream.jsonl" "git version" "the git version" || return 1
    expect_line "$dir/log/stream.jsonl" ok "the login shell output" || return 1
}

check_stub_oneshot() {
    local dir expected
    dir="$(session_dir stub-oneshot)"
    expected="$(result_count "$FIXTURE")"
    run_once "$STUB_IMAGE" "$dir" \
        claude -p hello --output-format stream-json --verbose \
        --permission-mode bypassPermissions --permission-prompts none \
        || { fail "the stub exited non-zero"; return 1; }
    head -n 1 "$dir/log/stream.jsonl" >"$dir/first.txt"
    expect_contains "$dir/first.txt" '"type":"system"' "the first line" || return 1
    expect_contains "$dir/first.txt" '"subtype":"init"' "the first line" || return 1
    expect_eq "$(result_count "$dir/log/stream.jsonl")" "$expected" \
        "result lines replayed from the fixture" || return 1
    expect_eq "$(init_count "$dir/log/stream.jsonl")" "$expected" \
        "init lines, one per replayed turn" || return 1
}

check_stub_interactive() {
    local dir stream code
    dir="$(session_dir stub-interactive)"
    write_mcp_config "$dir"
    stream="$dir/log/stream.jsonl"
    start_interactive_stub "$dir" 0 "${CONVERSATIONAL_ARGV[@]}"
    # Nothing is written before the first stdin line, `init` included, as the
    # real CLI does (ADR 0032; ARCHITECTURE.md, "Launch sequence").
    expect_silent "$stream" 3 "the stream before the first stdin line" || return 1
    send_user_line || return 1
    wait_for_line "$stream" '"subtype":"init"' 30 || return 1
    head -n 1 "$stream" >"$dir/init.txt"
    expect_contains "$dir/init.txt" '{"name":"mars-orchestrator","status":"connected"}' \
        "the mcp_servers of the init line" || return 1
    wait_for_line "$stream" '"type":"result"' 30 1 || return 1
    # A second relay, after the first has ended: the same process answers, so
    # the fixture's second turn follows its first and both carry one session id.
    send_user_line || return 1
    wait_for_line "$stream" '"type":"result"' 30 2 || return 1
    expect_eq "$(init_count "$stream")" 2 \
        "init lines after two turns (one opens each turn)" || return 1
    expect_eq "$(grep -F '"subtype":"init"' "$stream" | grep -o '"session_id":"[^"]*"' | sort -u | wc -l)" 1 \
        "distinct session ids across the two turns (one process)" || return 1
    # Both relays are gone and the CLI is still there: no writer leaving is an
    # EOF. It is ended the way Mars ends it, with a signal.
    expect_eq "$("$ENGINE" inspect --format '{{.State.Running}}' "$BG_NAME")" true \
        "the container running after both relays ended" || return 1
    "$ENGINE" kill --signal=SIGINT "$BG_NAME" >/dev/null
    code="$(timeout 30 "$ENGINE" wait "$BG_NAME")" \
        || { fail "the container did not exit within 30s of SIGINT"; return 1; }
    expect_eq "$code" 0 "exit code after SIGINT" || return 1
}

# start_signal_stub <dir> <stream> — the shared opening of the two signal
# checks: the stub mid-turn, paced at 300 ms a line, with the CLI as PID 1.
start_signal_stub() {
    local dir=$1 stream=$2 cmdline
    write_mcp_config "$dir"
    start_interactive_stub "$dir" 300 "${CONVERSATIONAL_ARGV[@]}"
    # The turn, `init` first, starts only once a line has been written (ADR 0032).
    send_user_line || return 1
    wait_for_line "$stream" '"subtype":"init"' 30 || return 1
    wait_for_lines "$stream" 2 30 || return 1
    cmdline="$("$ENGINE" exec "$BG_NAME" cat /proc/1/cmdline | tr '\0' ' ')"
    case "$cmdline" in
        *claude*) ;;
        *) fail "PID 1 is not the agent CLI"; return 1 ;;
    esac
}

check_stub_sigint() {
    local dir stream code
    dir="$(session_dir stub-sigint)"
    stream="$dir/log/stream.jsonl"
    start_signal_stub "$dir" "$stream" || return 1
    "$ENGINE" kill --signal=SIGINT "$BG_NAME" >/dev/null
    code="$(timeout 10 "$ENGINE" wait "$BG_NAME")" \
        || { fail "the container did not exit within 10s of SIGINT"; return 1; }
    expect_eq "$code" 0 "exit code after SIGINT" || return 1
    tail -n 2 "$stream" >"$dir/last.txt"
    expect_contains "$dir/last.txt" '[Request interrupted by user]' \
        "the interrupted user line of stream.jsonl" || return 1
    tail -n 1 "$stream" >"$dir/last.txt"
    expect_contains "$dir/last.txt" '"type":"result"' "the last line of stream.jsonl" || return 1
    expect_contains "$dir/last.txt" '"subtype":"error_during_execution"' \
        "the subtype of the closing result" || return 1
    expect_contains "$dir/last.txt" '"is_error":true' \
        "the is_error of the closing result" || return 1
    expect_contains "$dir/last.txt" '"terminal_reason":"aborted_streaming"' \
        "the terminal_reason of the closing result" || return 1
}

check_stub_sigterm() {
    local dir stream code before
    dir="$(session_dir stub-sigterm)"
    stream="$dir/log/stream.jsonl"
    start_signal_stub "$dir" "$stream" || return 1
    before="$(result_count "$stream")"
    "$ENGINE" kill --signal=SIGTERM "$BG_NAME" >/dev/null
    code="$(timeout 10 "$ENGINE" wait "$BG_NAME")" \
        || { fail "the container did not exit within 10s of SIGTERM"; return 1; }
    expect_eq "$code" 143 "exit code after SIGTERM" || return 1
    expect_eq "$(result_count "$stream")" "$before" \
        "result lines in stream.jsonl after SIGTERM" || return 1
}

# --- preflight and main -----------------------------------------------------

preflight() {
    local image
    if ! command -v "$ENGINE" >/dev/null 2>&1; then
        printf 'FAIL: preflight: no %s on PATH (set ENGINE=docker or ENGINE=podman)\n' "$ENGINE"
        exit 1
    fi
    for image in "$CLAUDE_IMAGE" "$STUB_IMAGE"; do
        if ! "$ENGINE" image inspect "$image" >/dev/null 2>&1; then
            printf 'FAIL: preflight: %s has no image %s; build it from %s first\n' \
                "$ENGINE" "$image" "$SCRIPT_DIR"
            exit 1
        fi
    done
    if [ ! -f "$CLAUDE_DOCKERFILE" ]; then
        printf 'FAIL: preflight: %s not found\n' "$CLAUDE_DOCKERFILE"
        exit 1
    fi
    if [ ! -f "$FIXTURE" ]; then
        printf 'FAIL: preflight: %s not found\n' "$FIXTURE"
        exit 1
    fi
    if [ "$ENGINE" = podman ] \
        && ! "$ENGINE" run --rm --userns=keep-id:uid=1000,gid=1000 "$STUB_IMAGE" true >/dev/null 2>&1; then
        printf 'FAIL: preflight: this Podman rejected --userns=keep-id:uid=1000,gid=1000, which the uid contract requires (ARCHITECTURE.md, "Uid contract"); try "podman system migrate" and check XDG_RUNTIME_DIR\n'
        exit 1
    fi
}

main() {
    case "${1:-}" in
        --help | -h)
            usage
            exit 0
            ;;
        "") ;;
        *)
            usage >&2
            exit 2
            ;;
    esac

    preflight

    local base start end redirect_dir
    if [ -n "${SMOKE_KEEP_DIR:-}" ]; then
        base="$SMOKE_KEEP_DIR"
        mkdir -p "$base"
    elif [ "$ENGINE" = podman ]; then
        # Rootless Podman needs the bind sources under the runner's own home.
        base="$HOME"
    else
        base="${TMPDIR:-/tmp}"
    fi
    TMP="$(mktemp -d "$base/smoke-XXXXXX")"
    [ "$ENGINE" = podman ] || chmod 0777 "$TMP"

    printf 'engine=%s claude=%s stub=%s\n' "$ENGINE" "$CLAUDE_IMAGE" "$STUB_IMAGE"
    start=$(date +%s%N)

    check entrypoint-redirect-claude \
        check_entrypoint_redirect "$CLAUDE_IMAGE" "$(session_dir redirect-claude)"
    redirect_dir="$(session_dir redirect-stub)"
    check entrypoint-redirect-stub check_entrypoint_redirect "$STUB_IMAGE" "$redirect_dir"
    check entrypoint-append check_entrypoint_append "$redirect_dir"
    check entrypoint-noargs check_entrypoint_noargs
    check claude-version check_claude_version
    check stub-oneshot check_stub_oneshot
    check stub-interactive check_stub_interactive
    check stub-sigint check_stub_sigint
    check stub-sigterm check_stub_sigterm

    end=$(date +%s%N)
    printf '%s checks, %s failed, %s ms total\n' \
        "$CHECKS" "$FAILURES" "$(((end - start) / 1000000))"
    [ "$FAILURES" -eq 0 ]
}

trap cleanup EXIT
main "$@"
