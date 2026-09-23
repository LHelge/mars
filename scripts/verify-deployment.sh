#!/usr/bin/env bash
#
# Mars deployment smoke test.
#
# Runs against a started compose stack (README.md, "Start") and asserts the
# deployment properties that are otherwise a manual checklist:
#
#   1. health     /api/health is 200 through nginx and says orchestrator: true
#   2. host       neither API_PORT nor MCP_PORT is published on the host
#   3. frontend   from mars-frontend the MCP listener answers only with a bearer
#                 challenge, while the API answers 200
#   4. sessions   from mars-sessions the `orchestrator` alias resolves and the
#                 MCP listener answers there too
#   5. logs       nginx access log lines for /ws/ and /tasks/stream carry no
#                 query string (SPEC.md, "Authentication")
#
# One `ok` / `warn` / `FAIL` line per check; non-zero exit if any check failed.
# Idempotent: running it twice passes twice.
#
# Environment:
#   ENGINE                 podman | docker (default: podman if present, else docker)
#   HTTP_PORT MCP_PORT API_PORT
#                          HTTP_PORT may be `address:port`, as in compose;
#                          nginx is then checked on that address
#                          read from ./.env when not already set; defaults
#                          8080 / 7001 / 7000 (README.md, "Configuration")
#   CURL_IMAGE             image used for the in-network probes (default
#                          docker.io/curlimages/curl:latest; override when air-gapped)
#   COMPOSE_FILE COMPOSE_PROJECT_NAME
#                          read from ./.env when not already set, so the log
#                          check reads the stack that was started
#   COMPOSE_CMD            override the compose command (e.g. "podman-compose")
#   ENV_FILE               alternative to ./.env
#   HOSTRUN=1              orchestrator runs on the host instead of in compose
#                          (README.md, "Podman setup" -> "Running the
#                          orchestrator on the host"): check 2 is inverted (the
#                          ports are expected open on the loopback address),
#                          check 3 probes the host gateway instead of the
#                          `orchestrator` compose alias, and check 4 is skipped
#                          because no orchestrator sits on mars-sessions
#
# No real credential is used or printed; the canary below is a fixed fake string.

set -euo pipefail

CANARY_TOKEN="VERIFY-CANARY-TOKEN"
CONTROL_CANARY="CANARY-CONTROL"
NIL_UUID="00000000-0000-0000-0000-000000000000"

failures=0

ok() { printf 'ok    %s\n' "$*"; }
warn() { printf 'warn  %s\n' "$*"; }
fail() {
    printf 'FAIL  %s\n' "$*"
    failures=$((failures + 1))
}

# --- configuration -----------------------------------------------------------

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
ENV_FILE=${ENV_FILE:-$repo_root/.env}

# Take VAR from the environment, else from ENV_FILE, else from the default.
# Deliberately not `source`: .env is data, not script.
env_default() {
    local var=$1 fallback=$2 value
    if [[ -n ${!var:-} ]]; then
        return
    fi
    value=""
    if [[ -f $ENV_FILE ]]; then
        value=$(sed -n "s/^[[:space:]]*\(export[[:space:]]\+\)\?${var}=//p" "$ENV_FILE" | tail -n 1)
        value=${value%$'\r'}
        value=${value%"${value##*[![:space:]]}"}
        if [[ $value == \"*\" || $value == \'*\' ]]; then
            value=${value:1:${#value}-2}
        fi
    fi
    [[ -n $value ]] || value=$fallback
    printf -v "$var" '%s' "$value"
    export "${var?}"
}

if [[ -z ${ENGINE:-} ]]; then
    if command -v podman >/dev/null 2>&1; then
        ENGINE=podman
    else
        ENGINE=docker
    fi
fi
case $ENGINE in
podman | docker) ;;
*)
    printf 'ENGINE must be podman or docker, got %s\n' "$ENGINE" >&2
    exit 2
    ;;
esac
command -v "$ENGINE" >/dev/null 2>&1 || {
    printf '%s is not installed\n' "$ENGINE" >&2
    exit 2
}

env_default HTTP_PORT 8080
env_default MCP_PORT 7001
env_default API_PORT 7000
env_default COMPOSE_FILE ""
env_default COMPOSE_PROJECT_NAME ""
# Fully qualified: Podman refuses to resolve a short name without a registries
# configuration, and Docker accepts the long form just as well.
CURL_IMAGE=${CURL_IMAGE:-docker.io/curlimages/curl:latest}
HOSTRUN=${HOSTRUN:-0}
# HTTP_PORT may carry an address, as compose accepts (`192.0.2.50:8080`,
# README.md "Configuration"): nginx then listens there and not on loopback.
if [[ $HTTP_PORT == *:* ]]; then
    HTTP_HOST=${HTTP_PORT%:*}
    HTTP_PORT=${HTTP_PORT##*:}
else
    HTTP_HOST=127.0.0.1
fi

printf 'engine=%s http_port=%s api_port=%s mcp_port=%s hostrun=%s\n' \
    "$ENGINE" "$HTTP_PORT" "$API_PORT" "$MCP_PORT" "$HOSTRUN"

# --- check 1: health through nginx -------------------------------------------

check_health() {
    local body
    if ! body=$(curl -fsS --max-time 10 "http://${HTTP_HOST}:${HTTP_PORT}/api/health" 2>/dev/null); then
        fail "health: GET http://${HTTP_HOST}:${HTTP_PORT}/api/health did not return 200"
        return
    fi
    if [[ $body == *'"orchestrator":true'* || $body == *'"orchestrator": true'* ]]; then
        ok "health: 200 through nginx on ${HTTP_PORT}, orchestrator true"
    else
        fail "health: 200 but no \"orchestrator\":true in the body: ${body}"
    fi
}

# --- check 2: nothing published on the host ----------------------------------

# 0 when the port refuses the connection (curl exit 7), 1 when something answered.
port_closed() {
    local url=$1 rc=0
    curl -s -o /dev/null --max-time 3 "$url" || rc=$?
    [[ $rc -eq 7 ]]
}

check_host_isolation() {
    local mcp_url="http://127.0.0.1:${MCP_PORT}/mcp"
    local api_url="http://127.0.0.1:${API_PORT}/api/health"
    local mcp_closed=1 api_closed=1
    if port_closed "$mcp_url"; then mcp_closed=0; fi
    if port_closed "$api_url"; then api_closed=0; fi

    if [[ $HOSTRUN == 1 ]]; then
        # A host-run orchestrator listens on loopback itself; that is the point.
        if [[ $mcp_closed -eq 1 ]]; then
            ok "host: MCP_PORT ${MCP_PORT} answers on loopback (HOSTRUN)"
        else
            fail "host: nothing answers on 127.0.0.1:${MCP_PORT} but HOSTRUN=1"
        fi
        if [[ $api_closed -eq 1 ]]; then
            ok "host: API_PORT ${API_PORT} answers on loopback (HOSTRUN)"
        else
            fail "host: nothing answers on 127.0.0.1:${API_PORT} but HOSTRUN=1"
        fi
        return
    fi

    if [[ $mcp_closed -eq 0 ]]; then
        ok "host: MCP_PORT ${MCP_PORT} is not reachable from the host"
    else
        fail "host: something answered on 127.0.0.1:${MCP_PORT} — the MCP port got published"
    fi
    if [[ $api_closed -eq 0 ]]; then
        ok "host: API_PORT ${API_PORT} is not reachable from the host"
    else
        fail "host: something answered on 127.0.0.1:${API_PORT} — the API port got published"
    fi
}

# --- checks 3 and 4: in-network probes ---------------------------------------

network_exists() {
    "$ENGINE" network inspect "$1" >/dev/null 2>&1
}

# Where a container on mars-frontend finds the orchestrator: the compose service
# alias normally, the host gateway when the orchestrator runs on the host — the
# same name and the same `--add-host` nginx gets from compose.hostrun.yml.
PROBE_HOST=orchestrator
PROBE_ARGS=()
if [[ $HOSTRUN == 1 ]]; then
    PROBE_HOST=host.containers.internal
    PROBE_ARGS=(--add-host "host.containers.internal:host-gateway")
fi

# Prints the HTTP status code, or the empty string when the request failed.
probe() {
    local network=$1 url=$2
    "$ENGINE" run --rm --network "$network" "${PROBE_ARGS[@]}" "$CURL_IMAGE" \
        -s -o /dev/null -m 10 -w '%{http_code}' "$url" 2>/dev/null || true
}

pull_curl_image() {
    # mars-sessions is internal: true, so a container started on it cannot pull.
    if "$ENGINE" image inspect "$CURL_IMAGE" >/dev/null 2>&1; then
        return
    fi
    if ! "$ENGINE" pull "$CURL_IMAGE" >/dev/null 2>&1; then
        warn "probe image ${CURL_IMAGE} could not be pulled; checks 3 and 4 will fail"
    fi
}

# The MCP router answers 401 without a bearer token (ARCHITECTURE.md, "MCP
# design"). A 404 means the scaffolding placeholder router is still in place:
# worth a warning, not a failure. A 200 is always a failure — it would mean the
# listener serves anybody who can open the port.
judge_mcp() {
    local label=$1 code=$2
    case $code in
    401) ok "${label}: /mcp answers 401 without a token" ;;
    404) warn "${label}: /mcp answers 404 — MCP placeholder router still in place" ;;
    200) fail "${label}: /mcp answers 200 without a token" ;;
    "") fail "${label}: /mcp probe could not run (image or network missing?)" ;;
    *) fail "${label}: /mcp answered ${code}, expected 401" ;;
    esac
}

check_frontend_network() {
    if ! network_exists mars-frontend; then
        if [[ $HOSTRUN == 1 ]]; then
            warn "frontend: network mars-frontend does not exist (HOSTRUN), skipped"
            return
        fi
        fail "frontend: network mars-frontend does not exist — is the stack up?"
        return
    fi
    # The MCP listener binds all interfaces and the orchestrator sits on both
    # networks (ARCHITECTURE.md, "Components"), so TCP reachability from
    # mars-frontend is expected. What must hold is that nothing there can *use*
    # it: no session token, so a bearer challenge (ARCHITECTURE.md, "Networks").
    # With HOSTRUN=1 the listener is on the host gateway instead, where it is
    # even more exposed — so this is the check that matters most in that mode.
    judge_mcp "frontend" "$(probe mars-frontend "http://${PROBE_HOST}:${MCP_PORT}/mcp")"

    local code
    code=$(probe mars-frontend "http://${PROBE_HOST}:${API_PORT}/api/health")
    if [[ $code == 200 ]]; then
        ok "frontend: /api/health answers 200 on mars-frontend"
    else
        fail "frontend: /api/health answered '${code}' on mars-frontend, expected 200"
    fi
}

check_sessions_network() {
    if [[ $HOSTRUN == 1 ]]; then
        warn "sessions: skipped (HOSTRUN=1)"
        return
    fi
    if ! network_exists mars-sessions; then
        fail "sessions: network mars-sessions does not exist — is the stack up?"
        return
    fi
    # Proves MCP_URL's default hostname resolves on the internal network and the
    # listener is reachable there (README.md, "Configuration").
    judge_mcp "sessions" "$(probe mars-sessions "http://orchestrator:${MCP_PORT}/mcp")"
}

# --- check 5: nginx log hygiene ----------------------------------------------

# Fills the COMPOSE array with a working compose command, or leaves it empty.
COMPOSE=()
detect_compose() {
    if [[ -n ${COMPOSE_CMD:-} ]]; then
        read -r -a COMPOSE <<<"$COMPOSE_CMD"
        return
    fi
    local candidates=()
    if [[ $ENGINE == podman ]]; then
        candidates=("podman compose" "podman-compose" "docker compose")
    else
        candidates=("docker compose" "docker-compose" "podman compose")
    fi
    local candidate parts
    for candidate in "${candidates[@]}"; do
        read -r -a parts <<<"$candidate"
        command -v "${parts[0]}" >/dev/null 2>&1 || continue
        if "${parts[@]}" version >/dev/null 2>&1; then
            COMPOSE=("${parts[@]}")
            return
        fi
    done
}

# Prints the nginx access+error log, most recent last.
nginx_logs() {
    local out
    if [[ ${#COMPOSE[@]} -gt 0 ]]; then
        if out=$("${COMPOSE[@]}" logs --no-log-prefix --tail 200 nginx 2>/dev/null) && [[ -n $out ]]; then
            printf '%s\n' "$out"
            return 0
        fi
        # podman-compose has no --no-log-prefix.
        if out=$("${COMPOSE[@]}" logs --tail 200 nginx 2>/dev/null) && [[ -n $out ]]; then
            printf '%s\n' "$out"
            return 0
        fi
    fi
    # Fallback: the nginx container of this stack, straight from the engine.
    local name
    name=$("$ENGINE" ps --format '{{.Names}}' 2>/dev/null | grep -m 1 nginx || true)
    if [[ -n $name ]] && out=$("$ENGINE" logs --tail 200 "$name" 2>&1) && [[ -n $out ]]; then
        printf '%s\n' "$out"
        return 0
    fi
    return 1
}

check_log_hygiene() {
    local base="http://${HTTP_HOST}:${HTTP_PORT}"
    # Both requests are rejected (401, or 404 for the unknown id); only the log
    # line matters. The token is a fixed fake string.
    curl -s -o /dev/null --max-time 5 \
        "${base}/ws/sessions/${NIL_UUID}?after=0&token=${CANARY_TOKEN}" || true
    curl -s -o /dev/null --max-time 5 \
        "${base}/api/projects/${NIL_UUID}/tasks/stream?token=${CANARY_TOKEN}" || true
    # Control: an ordinary /api/ request logs with the default format, so its
    # query string *must* show up. It proves the grep reads the right log and
    # that only the two stream locations are query-free.
    curl -s -o /dev/null --max-time 5 "${base}/api/health?x=${CONTROL_CANARY}" || true
    sleep 1

    local logs
    if ! logs=$(nginx_logs); then
        fail "logs: no nginx log lines found"
        return
    fi
    local recent stream_lines
    recent=$(printf '%s\n' "$logs" | tail -n 50)

    if ! printf '%s\n' "$recent" | grep -q "${CONTROL_CANARY}"; then
        fail "logs: the control request /api/health?x=${CONTROL_CANARY} is not in the last 50 lines — wrong log source"
        return
    fi
    ok "logs: control request with a query string is logged (the grep reads the right log)"

    stream_lines=$(printf '%s\n' "$recent" | grep -E "/ws/sessions/|/tasks/stream" || true)
    if [[ -z $stream_lines ]]; then
        fail "logs: neither /ws/ nor /tasks/stream appears in the last 50 lines"
        return
    fi
    local missing=""
    if ! printf '%s\n' "$stream_lines" | grep -q "/ws/sessions/${NIL_UUID}"; then
        missing+=" /ws/sessions/"
    fi
    if ! printf '%s\n' "$stream_lines" | grep -q "/tasks/stream"; then
        missing+=" /tasks/stream"
    fi
    if [[ -n $missing ]]; then
        fail "logs: missing stream request lines:${missing}"
        return
    fi

    # The stream locations log the `noquery` format (nginx/nginx.conf): $uri
    # only, so no '?', no `token=` and no token value can appear.
    local dirty=""
    if printf '%s\n' "$stream_lines" | grep -qF -- "$CANARY_TOKEN"; then
        dirty+=" the canary token"
    fi
    if printf '%s\n' "$stream_lines" | grep -qF -- 'token='; then
        dirty+=" token="
    fi
    if printf '%s\n' "$stream_lines" | grep -qF -- '?'; then
        dirty+=" ?"
    fi
    if [[ -n $dirty ]]; then
        fail "logs: stream log lines contain:${dirty}"
        printf '%s\n' "$stream_lines" | sed 's/^/      /' >&2
        return
    fi
    ok "logs: /ws/ and /tasks/stream lines carry no query string"
}

# --- run ---------------------------------------------------------------------

detect_compose
pull_curl_image

check_health
check_host_isolation
check_frontend_network
check_sessions_network
check_log_hygiene

if [[ $failures -gt 0 ]]; then
    printf '\n%d check(s) FAILED\n' "$failures"
    exit 1
fi
printf '\nall checks passed\n'
