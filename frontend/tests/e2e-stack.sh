#!/usr/bin/env bash
#
# The end-to-end stack: Postgres, the stub session image and a real
# orchestrator built with `--features integration-tests`, running on the host
# exactly as README.md, "Running locally", describes (DATA_DIR == DATA_DIR_HOST,
# MCP over host.containers.internal). `up` writes the connection facts to
# frontend/.e2e/env, which playwright.config.ts loads, so `npm run test:e2e`
# needs no hand-set variables.
#
# Usage: tests/e2e-stack.sh up|down|status
#
# Knobs: E2E_ENGINE (podman|docker), E2E_PG_PORT, E2E_PG_CONTAINER, E2E_API_PORT,
# E2E_MCP_PORT, E2E_BASE_URL, E2E_STUB_IMAGE, E2E_SKIP_IMAGE_BUILD, E2E_SKIP_CARGO_BUILD,
# E2E_KEEP_LOG. README.md, "Development" → "End-to-end tests".

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FRONTEND_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd "$FRONTEND_DIR/.." && pwd)"

E2E_DIR="$FRONTEND_DIR/.e2e"
DATA_DIR_PATH="$E2E_DIR/data"
REPOS_DIR_PATH="$E2E_DIR/repos"
RUN_DIR="$E2E_DIR/run"
LOG_FILE="$E2E_DIR/orchestrator.log"
PID_FILE="$E2E_DIR/orchestrator.pid"
ENV_FILE="$E2E_DIR/env"

# One stack per container name: a second stack on the same engine (a parallel
# agent, a second checkout) names its own.
PG_CONTAINER="${E2E_PG_CONTAINER:-mars-e2e-pg}"
PG_IMAGE="docker.io/library/postgres:18"

ENGINE_NAME="${E2E_ENGINE:-podman}"
PG_PORT="${E2E_PG_PORT:-5433}"
API_PORT="${E2E_API_PORT:-7000}"
MCP_PORT="${E2E_MCP_PORT:-7001}"
BASE_URL="${E2E_BASE_URL:-http://localhost:5173}"
STUB_IMAGE="${E2E_STUB_IMAGE:-localhost/mars-session-stub:dev}"

API_URL="http://localhost:$API_PORT"
HEALTH_URL="$API_URL/api/health"

# Seconds `up` waits for `GET /api/health` to answer 200.
HEALTH_TIMEOUT=180
# Seconds `down` waits after SIGTERM before SIGKILL.
STOP_TIMEOUT=10

say() { printf '%s\n' "$*"; }
fail() {
    printf 'e2e-stack: %s\n' "$*" >&2
    exit 1
}

# --- preconditions ----------------------------------------------------------

require_command() {
    command -v "$1" >/dev/null 2>&1 || fail "$1 is not on PATH; $2"
}

# The engine socket URL the orchestrator and this script both use. An already
# exported DOCKER_HOST wins, so a non-standard socket keeps working.
engine_socket() {
    if [ -n "${DOCKER_HOST:-}" ]; then
        printf '%s' "$DOCKER_HOST"
    elif [ "$ENGINE_NAME" = "docker" ]; then
        printf '%s' "unix:///var/run/docker.sock"
    else
        printf '%s' "unix://${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/podman/podman.sock"
    fi
}

port_free() {
    # bash's own /dev/tcp: a refused connection means nothing listens there.
    ! (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null
}

check_preconditions() {
    case "$ENGINE_NAME" in
    podman | docker) ;;
    *) fail "E2E_ENGINE must be podman or docker, not '$ENGINE_NAME'" ;;
    esac

    require_command "$ENGINE_NAME" "install it or set E2E_ENGINE"
    require_command cargo "the orchestrator is built from source"
    require_command git "the orchestrator shells out to git"
    require_command openssl "a fresh secrets master key is generated per run"
    require_command curl "the health endpoint is polled with it"

    DOCKER_HOST="$(engine_socket)"
    export DOCKER_HOST
    "$ENGINE_NAME" info >/dev/null 2>&1 ||
        fail "$ENGINE_NAME cannot reach its socket ($DOCKER_HOST); start it and retry"

    if [ "$ENGINE_NAME" = "podman" ]; then
        # keep-id maps this user to uid 1000 inside the container and needs
        # subordinate ids: without them the map has the single root line
        # (ARCHITECTURE.md, "Uid contract").
        # On macOS the containers run in the Podman machine's VM, so that is
        # where the map is read; `podman unshare` is not available remotely.
        local uid_map_lines unshare=(podman unshare)
        [ "$(uname -s)" = "Darwin" ] && unshare=(podman machine ssh -- podman unshare)
        uid_map_lines="$("${unshare[@]}" cat /proc/self/uid_map 2>/dev/null | grep -c . || true)"
        [ "${uid_map_lines:-0}" -gt 1 ] ||
            fail "rootless podman has no subordinate ids (see /etc/subuid); keep-id cannot map uid 1000"
    elif [ "$(id -u)" != "1000" ]; then
        say "e2e-stack: warning: Docker maps no user namespace and your uid is $(id -u), not 1000."
        say "e2e-stack: warning: the uid contract is not met, so session scenarios will fail (ARCHITECTURE.md, \"Uid contract\")."
    fi
}

check_ports() {
    local port
    for port in "$API_PORT" "$MCP_PORT" "$PG_PORT"; do
        port_free "$port" ||
            fail "port $port is already in use; set E2E_API_PORT, E2E_MCP_PORT or E2E_PG_PORT"
    done
}

# --- up ---------------------------------------------------------------------

start_postgres() {
    say "e2e-stack: starting $PG_CONTAINER on port $PG_PORT"
    "$ENGINE_NAME" rm -f "$PG_CONTAINER" >/dev/null 2>&1 || true
    "$ENGINE_NAME" run -d --name "$PG_CONTAINER" \
        -e POSTGRES_USER=mars -e POSTGRES_PASSWORD=mars -e POSTGRES_DB=mars \
        -p "$PG_PORT:5432" "$PG_IMAGE" >/dev/null

    local waited=0
    until "$ENGINE_NAME" exec "$PG_CONTAINER" pg_isready -U mars -d mars >/dev/null 2>&1; do
        waited=$((waited + 1))
        [ "$waited" -lt 60 ] || {
            "$ENGINE_NAME" logs --tail 50 "$PG_CONTAINER" >&2 || true
            fail "$PG_CONTAINER never became ready"
        }
        sleep 1
    done
}

build_stub_image() {
    if [ "${E2E_SKIP_IMAGE_BUILD:-0}" = "1" ]; then
        say "e2e-stack: skipping the stub image build (E2E_SKIP_IMAGE_BUILD=1)"
        return
    fi
    say "e2e-stack: building $STUB_IMAGE"
    "$ENGINE_NAME" build -t "$STUB_IMAGE" "$REPO_ROOT/images/stub" >/dev/null ||
        fail "building the stub image failed; rerun without output suppressed: $ENGINE_NAME build -t $STUB_IMAGE $REPO_ROOT/images/stub"
}

orchestrator_binary() {
    printf '%s' "${CARGO_TARGET_DIR:-$REPO_ROOT/orchestrator/target}/debug/mars-orchestrator"
}

build_orchestrator() {
    if [ "${E2E_SKIP_CARGO_BUILD:-0}" = "1" ]; then
        say "e2e-stack: skipping the cargo build (E2E_SKIP_CARGO_BUILD=1)"
    else
        say "e2e-stack: building the orchestrator with --features integration-tests"
        (cd "$REPO_ROOT/orchestrator" && cargo build --features integration-tests) ||
            fail "cargo build failed"
    fi
    [ -x "$(orchestrator_binary)" ] || fail "no orchestrator binary at $(orchestrator_binary)"
}

start_orchestrator() {
    mkdir -p "$DATA_DIR_PATH" "$REPOS_DIR_PATH" "$RUN_DIR"

    local mcp_host extra_hosts
    if [ "$ENGINE_NAME" = "docker" ]; then
        mcp_host="host.docker.internal"
    else
        mcp_host="host.containers.internal"
    fi
    extra_hosts="$mcp_host:host-gateway"

    # A fresh key per run: obviously fake, and never printed (CLAUDE.md, rule 3).
    local master_key
    master_key="1=$(openssl rand -base64 32)"

    # The log is appended to, and E2E_KEEP_LOG can leave an earlier run's in
    # place: `assert_logged_email` reads this run's lines only.
    LOG_START=0
    [ ! -f "$LOG_FILE" ] || LOG_START="$(wc -l <"$LOG_FILE")"

    say "e2e-stack: starting the orchestrator on $API_URL (log: $LOG_FILE)"
    # `env -i` keeps the developer's own environment out of the run, and the
    # working directory is two levels below `frontend/`, so `Config::from_env`'s
    # `.env`/`../.env` lookup cannot reach the repository's `.env`: this stack
    # sets every variable itself. The five the orchestrator reads and this stack
    # has no value for are pinned empty, which `Config` reads as unset and which
    # no `.env` overrides, so even a stray `.env` inside `.e2e/` cannot switch
    # the run to real mail delivery, another master key or a session network.
    # `assert_logged_email` below is the check that it held.
    #
    # Two job intervals are set away from their defaults. `MIRROR_FETCH_INTERVAL_SECS`
    # is long because nothing here waits on a fetch and every one of them touches
    # a `file://` upstream a scenario may be rewriting. `DISPATCHER_INTERVAL_SECS`
    # is short because `dispatcher.spec.ts` waits on a launch nobody made: the
    # `task_events` wake-up is what normally starts one within a second, and this
    # timer is only the fallback under it (`ARCHITECTURE.md`, "Dispatcher"). It
    # costs the rest of the suite nothing: every project's seeded implementer
    # and reviewer carry `auto_launch` (ADR 0051), but no scenario stores an
    # agent credential they could use outside its own project, so a sweep
    # launches nothing anywhere else (`tests/README.md`, "Automation and the
    # seeded roles").
    (
        cd "$RUN_DIR" &&
            exec env -i \
                PATH="$PATH" \
                HOME="$HOME" \
                RESEND_API_KEY= \
                MAIL_FROM= \
                SECRETS_MASTER_KEY_FILE= \
                SESSION_NETWORK_INTERNAL= \
                SESSION_NETWORK_EGRESS= \
                PUBLIC_URL="$BASE_URL" \
                JWT_SECRET="e2e-not-a-real-secret" \
                DATABASE_URL="postgres://mars:mars@localhost:$PG_PORT/mars" \
                DOCKER_HOST="$DOCKER_HOST" \
                DATA_DIR="$DATA_DIR_PATH" \
                DATA_DIR_HOST="$DATA_DIR_PATH" \
                MCP_URL="http://$mcp_host:$MCP_PORT/mcp" \
                SESSION_EXTRA_HOSTS="$extra_hosts" \
                SECRETS_MASTER_KEYS="$master_key" \
                GIT_BOT_NAME="Mars E2E Bot" \
                GIT_BOT_EMAIL="bot@example.test" \
                API_PORT="$API_PORT" \
                MCP_PORT="$MCP_PORT" \
                SESSION_IMAGE_DEFAULT="$STUB_IMAGE" \
                STOP_GRACE_SECS=5 \
                MIRROR_FETCH_INTERVAL_SECS=600 \
                DISPATCHER_INTERVAL_SECS=10 \
                RUST_LOG=info \
                "$(orchestrator_binary)" >>"$LOG_FILE" 2>&1
    ) &
    printf '%s\n' "$!" >"$PID_FILE"
}

await_health() {
    local pid code waited=0
    pid="$(cat "$PID_FILE")"
    while :; do
        code="$(curl -s -o "$RUN_DIR/health.json" -w '%{http_code}' "$HEALTH_URL" || true)"
        [ "$code" != "200" ] || return 0
        orchestrator_alive "$pid" || break
        [ "$waited" -lt "$HEALTH_TIMEOUT" ] || break
        waited=$((waited + 1))
        sleep 1
    done

    say "e2e-stack: $HEALTH_URL never answered 200 (last status: ${code:-none})"
    [ ! -s "$RUN_DIR/health.json" ] || say "e2e-stack: health: $(cat "$RUN_DIR/health.json")"
    say "e2e-stack: last 50 lines of $LOG_FILE:"
    tail -n 50 "$LOG_FILE" >&2 || true
    exit 1
}

# The scenarios that follow an invitation, a reset or an escalation read the
# link out of the orchestrator's log, which only `LogEmailClient` writes; with
# `RESEND_API_KEY` set the orchestrator would instead try to deliver the suite's
# mail through somebody's real account. Its own startup line says which client
# it chose, so a configuration that leaked in fails here, by name, and not as
# five timeouts an hour later. The orchestrator is stopped first: nothing may
# run against it. Only the line's presence is read, never a value (rule 3).
assert_logged_email() {
    if tail -n "+$((LOG_START + 1))" "$LOG_FILE" | grep -q "RESEND_API_KEY is unset"; then
        return 0
    fi

    stop_orchestrator
    fail "the orchestrator did not choose the logging email client, so RESEND_API_KEY reached it from somewhere this stack does not control (a .env in $E2E_DIR or $RUN_DIR?). The orchestrator has been stopped."
}

write_env_file() {
    cat >"$ENV_FILE" <<EOF
PLAYWRIGHT_BASE_URL=$BASE_URL
PLAYWRIGHT_API_URL=$API_URL
PLAYWRIGHT_ORCHESTRATOR_LOG=$LOG_FILE
PLAYWRIGHT_DATA_DIR=$DATA_DIR_PATH
PLAYWRIGHT_REPOS_DIR=$REPOS_DIR_PATH
PLAYWRIGHT_STUB_IMAGE=$STUB_IMAGE
PLAYWRIGHT_ENGINE=$ENGINE_NAME
EOF
}

cmd_up() {
    check_preconditions
    mkdir -p "$E2E_DIR"
    # A previous run's orchestrator and database are taken down first: they hold
    # the ports the checks below want, and a run never reuses a database.
    stop_orchestrator
    "$ENGINE_NAME" rm -f "$PG_CONTAINER" >/dev/null 2>&1 || true
    check_ports
    start_postgres
    build_stub_image
    build_orchestrator
    start_orchestrator
    await_health
    assert_logged_email
    write_env_file
    say "e2e-stack: up. $(cat "$RUN_DIR/health.json")"
    say "e2e-stack: wrote $ENV_FILE; sign in as admin/changeme"
}

# --- down -------------------------------------------------------------------

# Whether the recorded pid is still this stack's orchestrator. A pid file left
# by an earlier run can name a pid the kernel has since given to something else
# -- a developer's own orchestrator, or any other process -- and this stack
# never signals that.
pid_is_orchestrator() {
    local pid="$1" comm
    [ -n "$pid" ] || return 1
    kill -0 "$pid" 2>/dev/null || return 1
    comm="$(process_name "$pid")"
    # Linux truncates /proc/<pid>/comm to 15 characters.
    case "$comm" in
    mars-orchestr*) ;;
    *) return 1 ;;
    esac
    # The name alone also fits a developer's own orchestrator; only this
    # stack's runs from its run directory.
    [ -d "$RUN_DIR" ] || return 1
    [ "$(process_cwd "$pid")" = "$(cd "$RUN_DIR" && pwd -P)" ]
}

# A process's name, its working directory and its state letter: `/proc` on
# Linux, `ps` and `lsof` on macOS, which has no `/proc`.
process_name() {
    if [ -d /proc ]; then
        cat "/proc/$1/comm" 2>/dev/null || true
    else
        basename "$(ps -o comm= -p "$1" 2>/dev/null || true)"
    fi
}

process_cwd() {
    if [ -d /proc ]; then
        readlink "/proc/$1/cwd" 2>/dev/null || true
    else
        lsof -a -p "$1" -d cwd -Fn 2>/dev/null | sed -n 's/^n//p'
    fi
}

process_state() {
    if [ -d /proc ]; then
        sed 's/.*) //' "/proc/$1/stat" 2>/dev/null | cut -d' ' -f1
    else
        ps -o stat= -p "$1" 2>/dev/null | cut -c1
    fi
}

# Whether that pid is a live orchestrator rather than one this shell has yet to
# reap: an exited child stays visible to `kill -0` as a zombie, and `up` must
# notice a crash instead of polling health for three minutes.
orchestrator_alive() {
    local pid="$1" state
    pid_is_orchestrator "$pid" || return 1
    state="$(process_state "$pid")"
    [ "$state" != "Z" ]
}

stop_orchestrator() {
    [ -f "$PID_FILE" ] || return 0
    local pid waited=0
    pid="$(cat "$PID_FILE")"
    rm -f "$PID_FILE"
    if pid_is_orchestrator "$pid"; then
        say "e2e-stack: stopping the orchestrator (pid $pid)"
        kill -TERM "$pid" 2>/dev/null || true
        while kill -0 "$pid" 2>/dev/null; do
            [ "$waited" -lt "$STOP_TIMEOUT" ] || {
                kill -KILL "$pid" 2>/dev/null || true
                break
            }
            waited=$((waited + 1))
            sleep 1
        done
    fi
}

# Only the containers this stack started: a developer's own orchestrator on the
# same engine uses the same `mars.session_id` and `mars.probe` labels and the
# same `mars-session-<id>` names, so the data directory bind-mounted into each
# container is what tells the two apart.
remove_stack_containers() {
    local label id sources
    for label in mars.session_id mars.probe; do
        while read -r id; do
            [ -n "$id" ] || continue
            sources="$("$ENGINE_NAME" inspect --format '{{range .Mounts}}{{.Source}} {{end}}' "$id" 2>/dev/null || true)"
            case " $sources " in
            *" $DATA_DIR_PATH"*)
                say "e2e-stack: removing container $id"
                "$ENGINE_NAME" rm -f "$id" >/dev/null 2>&1 || true
                ;;
            esac
        done <<<"$("$ENGINE_NAME" ps -aq --filter "label=$label" 2>/dev/null || true)"
    done
}

cmd_down() {
    DOCKER_HOST="$(engine_socket)"
    export DOCKER_HOST
    stop_orchestrator
    if command -v "$ENGINE_NAME" >/dev/null 2>&1 && "$ENGINE_NAME" info >/dev/null 2>&1; then
        remove_stack_containers
        "$ENGINE_NAME" rm -f "$PG_CONTAINER" >/dev/null 2>&1 || true
    else
        say "e2e-stack: $ENGINE_NAME is unreachable; leaving containers alone"
    fi

    if [ -d "$E2E_DIR" ]; then
        if [ "${E2E_KEEP_LOG:-0}" = "1" ] && [ -f "$LOG_FILE" ]; then
            local kept
            kept="$(mktemp)"
            cp "$LOG_FILE" "$kept"
            rm -rf "$E2E_DIR"
            mkdir -p "$E2E_DIR"
            mv "$kept" "$LOG_FILE"
            say "e2e-stack: kept $LOG_FILE (E2E_KEEP_LOG=1)"
        else
            rm -rf "$E2E_DIR"
        fi
    fi
    say "e2e-stack: down"
}

# --- status -----------------------------------------------------------------

cmd_status() {
    DOCKER_HOST="$(engine_socket)"
    export DOCKER_HOST

    if command -v "$ENGINE_NAME" >/dev/null 2>&1 && "$ENGINE_NAME" info >/dev/null 2>&1; then
        say "engine:       $ENGINE_NAME up ($DOCKER_HOST)"
        local state
        state="$("$ENGINE_NAME" inspect --format '{{.State.Status}}' "$PG_CONTAINER" 2>/dev/null || true)"
        say "postgres:     ${state:-absent} ($PG_CONTAINER, port $PG_PORT)"
    else
        say "engine:       $ENGINE_NAME unreachable ($DOCKER_HOST)"
        say "postgres:     unknown"
    fi

    local pid="" code
    [ ! -f "$PID_FILE" ] || pid="$(cat "$PID_FILE")"
    if pid_is_orchestrator "$pid"; then
        say "orchestrator: running (pid $pid)"
    else
        say "orchestrator: down"
    fi

    code="$(curl -s -o /dev/null -w '%{http_code}' "$HEALTH_URL" || true)"
    if [ "$code" = "200" ]; then
        say "health:       200 $(curl -s "$HEALTH_URL")"
    else
        say "health:       ${code:-no answer} at $HEALTH_URL"
    fi

    if [ -f "$ENV_FILE" ]; then
        say "env file:     $ENV_FILE"
    else
        say "env file:     absent"
    fi
}

case "${1:-}" in
up) cmd_up ;;
down) cmd_down ;;
status) cmd_status ;;
*)
    say "usage: tests/e2e-stack.sh up|down|status"
    exit 2
    ;;
esac
