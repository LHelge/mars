#!/usr/bin/env bash
# shellcheck disable=SC2016 # jq filters name their own --arg variables as $x
# Transition tests of the deployment updater (ARCHITECTURE.md, "Server
# deployment"; README.md, "Automatic deployments"): real releases applied by
# the real bin/mars-deploy to a real stack on rootless Podman, one after the
# other, each asserting what the operator is promised. Fake credentials only.
#
#   TRANSITIONS_ORCHESTRATOR=<image> TRANSITIONS_NGINX=<image> \
#   TRANSITIONS_SESSION=<image> scripts/release/test-transitions.sh
#
# The three inputs are local images: an orchestrator and an nginx image built
# from this commit, and a session image (the stub is enough; nothing is
# launched in it, but the orchestrator's startup probe runs it). Each release
# below gets its own label-only variant of each, so every release has image
# digests of its own, and a throwaway registry:2 serves them. A
# registries.conf remap sends `ghcr.io/lhelge/…` to that registry, so the
# manifests carry the `ghcr.io/lhelge/<name>@sha256:` references the validator
# demands and the updater pulls them exactly as on a server. The bundles are
# built by make-bundle.sh from HEAD, or from the working tree when it has
# uncommitted changes (`git stash create`), and variants of them edit the
# manifest and the generated compose override.
#
# Everything runs under the compose project `marstrans`, a root in a scratch
# directory, session-image aliases under `localhost/marstrans-*`, HTTP on
# 127.0.0.1:$TRANSITIONS_HTTP_PORT (18090) and the registry on
# 127.0.0.1:$TRANSITIONS_REGISTRY_PORT (5057), so a developer's own stack and
# images are never touched. It refuses to start while any container labelled
# `mars.session_id` exists: the test orchestrator's orphan cleanup would
# remove it. Needs podman, podman-compose, jq, curl, flock, setsid and the
# Podman API socket ($XDG_RUNTIME_DIR/podman/podman.sock).
#
# Where `systemctl --user` works, the user units are tested as well: they are
# installed into the real user manager with `install-units --env` overrides
# and removed again at the end, so it refuses to start while a mars.service
# is already installed for this user. TRANSITIONS_UNITS=0 skips them.
set -uo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(git -C "$here" rev-parse --show-toplevel)

for var in TRANSITIONS_ORCHESTRATOR TRANSITIONS_NGINX TRANSITIONS_SESSION; do
  if [ -z "${!var:-}" ]; then
    echo "${var} is required: a local image (see the header)" >&2
    exit 2
  fi
done
socket="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/podman/podman.sock"
if [ ! -S "$socket" ]; then
  echo "no Podman API socket at ${socket}" >&2
  exit 2
fi
if [ -n "$(podman ps -aq --filter label=mars.session_id)" ]; then
  echo "session containers exist; the test orchestrator's orphan cleanup would remove them" >&2
  exit 2
fi

units_dir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
units=false
if [ "${TRANSITIONS_UNITS:-1}" != 0 ] && systemctl --user show-environment >/dev/null 2>&1; then
  units=true
  if [ -e "${units_dir}/mars.service" ]; then
    echo "${units_dir}/mars.service exists; this user runs Mars units already" >&2
    exit 2
  fi
fi

HTTP_PORT=${TRANSITIONS_HTTP_PORT:-18090}
REG=127.0.0.1:${TRANSITIONS_REGISTRY_PORT:-5057}
PROJECT=marstrans
work=$(mktemp -d)
export MARS_ROOT="${work}/root"
export MARS_PROJECT=$PROJECT
export MARS_REGISTRY="${REG}/lhelge"
export MARS_HEALTH_TIMEOUT=${TRANSITIONS_HEALTH_TIMEOUT:-90}
export SESSION_IMAGES_BASE_ALIAS="localhost/${PROJECT}-session-claude:latest"
export SESSION_IMAGES_DEV_ALIAS="localhost/${PROJECT}-session-claude-dev:latest"
export CONTAINERS_REGISTRIES_CONF="${work}/registries.conf"
cat >"$CONTAINERS_REGISTRIES_CONF" <<EOF
[[registry]]
prefix = "ghcr.io/lhelge"
location = "${REG}/lhelge"
insecure = true

[[registry]]
location = "${REG}"
insecure = true
EOF
ORCH="${PROJECT}_orchestrator_1"
PG="${PROJECT}_postgres_1"
NGINX="${PROJECT}_nginx_1"
LOCK="${MARS_ROOT}/state/lock"
ENV_FILE="${MARS_ROOT}/mars.env"
BASE="http://127.0.0.1:${HTTP_PORT}"

failures=0
pass() { echo "ok   $1"; }
fail() {
  echo "FAIL $1"
  failures=$((failures + 1))
}
# check <name> <command...>: passes when the command succeeds.
check() {
  local name=$1
  shift
  if "$@"; then pass "$name"; else fail "$name"; fi
}
not() { ! "$@"; }
section() { printf '\n--- %s\n' "$*"; }

pulled=()
units_installed=false
remove_units() {
  local u
  systemctl --user disable --now mars-deploy.timer mars-backup.timer mars.service >/dev/null 2>&1
  for u in mars.service mars-deploy.service mars-deploy.timer mars-backup.service mars-backup.timer mars-failed@.service; do
    rm -f "${units_dir}/${u}"
  done
  systemctl --user daemon-reload
  systemctl --user reset-failed 'mars*' >/dev/null 2>&1
}
cleanup() {
  local dir
  [ "$units_installed" = false ] || remove_units
  dir=$(readlink -f "${MARS_ROOT}/current" 2>/dev/null)
  if [ -n "$dir" ] && [ -d "$dir" ]; then
    (cd "$dir" && timeout 300 podman-compose -p "$PROJECT" -f compose.yml -f compose.podman.yml -f compose.release.yml \
      down -v -t 5 >/dev/null 2>&1)
  fi
  podman rm -f -t 0 "$ORCH" "$NGINX" "$PG" "${PROJECT}-registry" >/dev/null 2>&1
  podman volume rm -f "${PROJECT}_pgdata" >/dev/null 2>&1
  podman pod rm -f "pod_${PROJECT}" >/dev/null 2>&1
  podman rmi -f "$SESSION_IMAGES_BASE_ALIAS" "$SESSION_IMAGES_DEV_ALIAS" >/dev/null 2>&1
  [ "${#pulled[@]}" -eq 0 ] || podman rmi -f "${pulled[@]}" >/dev/null 2>&1
  podman images --format '{{.Repository}}:{{.Tag}}' | grep "^${REG}/" | xargs -r podman rmi -f >/dev/null 2>&1
  podman images --format '{{.Repository}}@{{.Digest}}' | grep "^${REG}/" | xargs -r podman rmi -f >/dev/null 2>&1
  podman unshare rm -rf "$work"
}
trap cleanup EXIT

# --- images and bundles ---------------------------------------------------------

podman rm -f -t 0 "${PROJECT}-registry" >/dev/null 2>&1
podman run -d --name "${PROJECT}-registry" -p "${REG}:5000" docker.io/library/registry:2 >/dev/null ||
  { echo "cannot start the registry" >&2; exit 1; }
for _ in $(seq 1 50); do curl -fs "http://${REG}/v2/" >/dev/null && break; sleep 0.2; done

mkdir -p "${work}/ctx"
# variant_image <package> <source image> <variant> -> ghcr.io/lhelge/<package>@<digest>
variant_image() {
  local tag="${REG}/lhelge/$1:$3"
  printf 'FROM %s\nLABEL mars.transitions.variant=%s\n' "$2" "$3" >"${work}/ctx/Containerfile"
  podman build -q --pull=never -t "$tag" "${work}/ctx" >/dev/null || return 1
  podman push -q --tls-verify=false --digestfile "${work}/digest" "$tag" >/dev/null || return 1
  printf 'ghcr.io/lhelge/%s@%s' "$1" "$(cat "${work}/digest")"
}
declare -A IMG=()
# image_set <name>: the four images of one release, IMG[<name>.<key>].
image_set() {
  if ! { IMG[$1.orchestrator]=$(variant_image mars-orchestrator "$TRANSITIONS_ORCHESTRATOR" "$1") &&
    IMG[$1.nginx]=$(variant_image mars-nginx "$TRANSITIONS_NGINX" "$1") &&
    IMG[$1.session_claude]=$(variant_image mars-session-claude "$TRANSITIONS_SESSION" "$1") &&
    IMG[$1.session_claude_dev]=$(variant_image mars-session-claude-dev "$TRANSITIONS_SESSION" "$1"); }; then
    echo "cannot build and push image set $1" >&2
    exit 1
  fi
  pulled+=("${IMG[$1.orchestrator]}" "${IMG[$1.nginx]}" "${IMG[$1.session_claude]}" "${IMG[$1.session_claude_dev]}")
}
for set in a b c d f k m n; do image_set "$set"; done

# On Podman 6, which accepts keep-id inside a pod, release A is a pod-era
# release: its compose.podman.yml lacks `x-podman: in_pod: false`, as every
# release before that line did, so the update to B is also the move out of
# podman-compose's pod that a server makes once. Podman 4.9 and 5 cannot
# start a pod-era release at all.
pod_era=false
[ "$(podman version --format '{{.Client.Version}}' | cut -d. -f1)" -ge 6 ] && pod_era=true

commit=$(git -C "$root" stash create)
[ -n "$commit" ] || commit=$(git -C "$root" rev-parse HEAD)
(cd "$root" && RELEASE_COMMIT=$commit RELEASE_RUN_ID=1 RELEASE_RUN_ATTEMPT=1 \
  IMAGE_ORCHESTRATOR=${IMG[a.orchestrator]} IMAGE_NGINX=${IMG[a.nginx]} \
  IMAGE_SESSION_CLAUDE=${IMG[a.session_claude]} IMAGE_SESSION_CLAUDE_DEV=${IMG[a.session_claude_dev]} \
  scripts/release/make-bundle.sh "${work}/base") >/dev/null || { echo "make-bundle.sh failed" >&2; exit 1; }
seq0=$(jq .source.sequence "${work}/base/bundle/manifest.json")

declare -A DIGEST=()
# bundle <name> <sequence offset> <image set> [jq filter] [broken]
#   A release bundle from the base one: its own commit and sequence, the
#   images of <image set>, the manifest edited by <jq filter>, and with
#   `broken` an orchestrator that exits at once (a subcommand that does not
#   exist), so it never becomes healthy. Pushed as mars-deploy:<name> and as
#   mars-deploy:sha-<commit>, as the Release workflow does.
bundle() {
  local name=$1 seq=$((seq0 + $2)) set=$3 filter=${4:-.} broken=${5:-} d="${work}/bundles/$1" c
  c=$(printf '%040x' "$seq")
  [ "$name" = a ] && c=$commit
  mkdir -p "${work}/bundles"
  cp -r "${work}/base" "$d"
  if [ "$name" = a ] && [ "$pod_era" = true ]; then
    sed -i '/^x-podman:/,/^  in_pod: false$/d' "${d}/bundle/compose.podman.yml"
  fi
  jq --arg c "$c" --argjson s "$seq" \
    --arg o "${IMG[$set.orchestrator]}" --arg n "${IMG[$set.nginx]}" \
    --arg sc "${IMG[$set.session_claude]}" --arg sd "${IMG[$set.session_claude_dev]}" \
    ".source.commit = \$c | .source.sequence = \$s
     | .images = {orchestrator: \$o, nginx: \$n, session_claude: \$sc, session_claude_dev: \$sd}
     | ${filter}" "${work}/base/bundle/manifest.json" >"${d}/bundle/manifest.json"
  {
    echo "services:"
    echo "  orchestrator:"
    echo "    image: $(jq -r .images.orchestrator "${d}/bundle/manifest.json")"
    [ -z "$broken" ] || echo '    command: ["no-such-subcommand"]'
    echo "  nginx:"
    echo "    image: $(jq -r .images.nginx "${d}/bundle/manifest.json")"
  } >"${d}/bundle/compose.release.yml"
  if ! { podman build -q -t "${MARS_REGISTRY}/mars-deploy:${name}" "$d" >/dev/null &&
    podman push -q --tls-verify=false --digestfile "${d}/digest" "${MARS_REGISTRY}/mars-deploy:${name}" >/dev/null &&
    retag "$(cat "${d}/digest")" "sha-${c}"; }; then
    echo "cannot publish bundle ${name}" >&2
    exit 1
  fi
  # A server never has the bundle in local storage before it pulls it, and
  # a locally built copy makes `image inspect` report a digest of its own
  # (Podman 4.9) instead of the pulled one.
  podman rmi -f "${MARS_REGISTRY}/mars-deploy:${name}" >/dev/null
  DIGEST[$name]=$(cat "${d}/digest")
}
# retag <digest> <tag>: points a tag of the bundle repository at a pushed
# manifest in the registry itself. Pushing the tag again from Podman is not
# the same: Podman 4.9 re-encodes the manifest, and the tag would name
# another digest than the one the test recorded.
retag() {
  local url="http://${REG}/v2/lhelge/mars-deploy/manifests" type
  curl -fsS -D "${work}/headers" -o "${work}/manifest" \
    -H 'Accept: application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json' \
    "${url}/$1" || return 1
  type=$(sed -n 's/^[Cc]ontent-[Tt]ype: *\([^;[:space:]]*\).*/\1/p' "${work}/headers")
  curl -fsS -X PUT -H "Content-Type: ${type}" --data-binary "@${work}/manifest" "${url}/$2" >/dev/null
}
promote() { retag "${DIGEST[$1]}" main; }

no_image="ghcr.io/lhelge/mars-nginx@sha256:$(printf '0%.0s' $(seq 1 64))"
bundle a 0 a
bundle b 1 b
bundle c 2 c
bundle x 3 c ".images.nginx = \"${no_image}\""
bundle d 4 d
bundle f 5 f . broken
bundle e 6 d '.epoch = 2 | .epoch_note = "Transitions test: nothing to do."'
bundle g 7 d '.epoch = 2 | .config.required += ["MARS_TRANSITIONS_REQUIRED"]'
bundle h 8 d '.epoch = 2 | .schema.migrations |= .[:-1]'
bundle i 9 d '.epoch = 2 | .postgres.major = 17'
bundle j 10 d '.epoch = 2 | .platform = "linux/arm64"'
bundle k 11 k '.epoch = 2'
bundle n 12 n '.epoch = 2'
bundle p 13 f '.epoch = 2' broken
bundle m 14 m '.epoch = 2 | .schema.migrations += ["29991231235959"]' broken

# --- the server -------------------------------------------------------------------

mkdir -p "${MARS_ROOT}/data" "${MARS_ROOT}/backups" "${MARS_ROOT}/releases"
chmod 700 "$MARS_ROOT"
podman pull -q docker.io/library/postgres:18 >/dev/null || { echo "cannot pull postgres:18" >&2; exit 1; }
pg_image="docker.io/library/postgres@$(podman image inspect --format '{{.Digest}}' docker.io/library/postgres:18)"
JWT="transitions-only-jwt-secret-$(openssl rand -hex 16)"
DBPW="transitions-only-db-password-$(openssl rand -hex 8)"
MKEY=$(openssl rand -base64 32)
cat >"${work}/notify" <<EOF
#!/bin/sh
printf '%s\t%s\n' "\$1" "\$2" >>"${work}/notified"
EOF
chmod 755 "${work}/notify"
cat >"$ENV_FILE" <<ENV
PUBLIC_URL=${BASE}
JWT_SECRET=${JWT}
POSTGRES_USER=mars
POSTGRES_PASSWORD=${DBPW}
POSTGRES_DB=mars
POSTGRES_IMAGE=${pg_image}
DOCKER_HOST=unix://${socket}
ENGINE_SOCKET_HOST=${socket}
DATA_DIR_HOST=${MARS_ROOT}/data
SECRETS_MASTER_KEYS=1=${MKEY}
GIT_BOT_NAME=Mars Transitions
GIT_BOT_EMAIL=mars-transitions@example.invalid
HTTP_PORT=127.0.0.1:${HTTP_PORT}
SESSION_IMAGE_DEFAULT=${SESSION_IMAGES_DEV_ALIAS}
MARS_BACKUP_DIR=${MARS_ROOT}/backups
MARS_DEPLOY_NOTIFY_HOOK=${work}/notify
ENV
chmod 600 "$ENV_FILE"
touch "${work}/notified"

# --- helpers -------------------------------------------------------------------------

n=0
# md <command...>: the installed updater; output to $work/log/<n>, status in $rc.
md() {
  n=$((n + 1))
  mkdir -p "${work}/log"
  "${MARS_ROOT}/current/bin/mars-deploy" "$@" >"${work}/log/${n}" 2>&1
  rc=$?
}
out() { cat "${work}/log/${n}"; }
said() { grep -qF -- "$1" "${work}/log/${n}"; }
st() { jq -r "$1" "${MARS_ROOT}/state/state.json"; }
cid() { podman inspect --format '{{.Id}}' "$1" 2>/dev/null; }
running() { [ "$(podman inspect --format '{{.State.Running}}' "$1" 2>/dev/null)" = true ]; }
image_id() { podman image inspect --format '{{.Id}}' "$1" 2>/dev/null; }
runs_image() { [ "$(podman inspect --format '{{.Image}}' "$1" 2>/dev/null)" = "$(image_id "$2")" ]; }
installed() { [ "$(readlink "${MARS_ROOT}/current")" = "releases/${DIGEST[$1]/:/-}" ] && [ "$(st .current.bundle)" = "${DIGEST[$1]}" ]; }
aliases_on() {
  [ "$(image_id "$SESSION_IMAGES_DEV_ALIAS")" = "$(image_id "${IMG[$1.session_claude_dev]}")" ] &&
    [ "$(image_id "$SESSION_IMAGES_BASE_ALIAS")" = "$(image_id "${IMG[$1.session_claude]}")" ]
}
healthy() {
  local body
  body=$(curl -fsS --max-time 5 "${BASE}/api/health" 2>/dev/null) &&
    jq -e '.orchestrator == true and .database == true and .engine == true' <<<"$body" >/dev/null
}
# eventually <seconds> <command...>: polls, for a state something else is
# still reaching (an orchestrator started again without a health wait).
eventually() {
  local deadline=$((SECONDS + $1))
  shift
  until "$@"; do
    [ "$SECONDS" -lt "$deadline" ] || return 1
    sleep 2
  done
}
serves_frontend() { curl -fsS --max-time 5 "${BASE}/" 2>/dev/null | grep -qi '<html'; }
psql_q() { podman exec "$PG" psql -At -U mars -d mars -c "$1" 2>/dev/null; }
identity() { psql_q "SELECT system_identifier FROM pg_control_system()" && psql_q "SELECT id FROM users WHERE username = 'admin'"; }
backups() { find "${MARS_ROOT}/backups" -mindepth 1 -maxdepth 1 -name '*-db-pre-deploy' | wc -l; }
# No process may hold the lock between runs: a container's conmon inheriting
# descriptor 9 would block every later run for as long as it lives.
lock_free() { [ -z "$(find /proc/[0-9]*/fd -lname "$LOCK" 2>/dev/null)" ]; }
failed_has() { jq -e --arg x "${DIGEST[$1]}" '.failed | index($x) != null' "${MARS_ROOT}/state/state.json" >/dev/null; }
notified() { grep -q "^$1	" "${work}/notified"; }
# On a wrong exit status: the output, and what the stack looked like.
exit_is() {
  [ "$rc" -eq "$1" ] && return 0
  echo "     exit ${rc}, want $1; output:"
  out | sed 's/^/     /'
  podman ps -a --filter "name=^${PROJECT}_" --format '{{.Names}} {{.Status}} {{.Image}}' | sed 's/^/     ps: /'
  podman logs --tail 40 "$ORCH" 2>&1 | sed 's/^/     orchestrator: /'
  # Once per run: the updater discards compose's output, so the attempted
  # release's orchestrator is started again here with it shown.
  if [ -z "${diagnosed:-}" ] && [ -f "${MARS_ROOT}/state/state.json" ] && [ "$(st '.attempted.target // ""')" != "" ]; then
    diagnosed=1
    (cd "${MARS_ROOT}/releases/$(st .attempted.target | tr : -)" &&
      timeout 120 podman-compose -p "$PROJECT" -f compose.yml -f compose.podman.yml -f compose.release.yml \
        up -d --no-deps orchestrator 2>&1 | tail -30 | sed 's/^/     compose: /')
  fi
  return 1
}

# --- 1. first install, the README's commands ----------------------------------------

section "1. first install (README.md, \"The first install\")"
promote a
digest=$(podman pull -q "${MARS_REGISTRY}/mars-deploy:main" >/dev/null &&
  podman image inspect --format '{{.Digest}}' "${MARS_REGISTRY}/mars-deploy:main")
rel=${MARS_ROOT}/releases/${digest/:/-}
c=$(podman create --entrypoint /none "${MARS_REGISTRY}/mars-deploy@${digest}")
podman cp "${c}:/bundle" "$rel" && podman rm "$c" >/dev/null
ln -s ../../mars.env "${rel}/.env"
n=$((n + 1))
mkdir -p "${work}/log"
"${rel}/bin/mars-deploy" deploy "$digest" >"${work}/log/${n}" 2>&1
rc=$?
check "the first deploy exits 0" exit_is 0
check "the resolved digest is release A" test "$digest" = "${DIGEST[a]}"
check "current links to release A" installed a
check "it is pinned" test "$(st .pin)" = "${DIGEST[a]}"
check "health through nginx is green" healthy
check "nginx serves the frontend" serves_frontend
check "the orchestrator runs A's image" runs_image "$ORCH" "${IMG[a.orchestrator]}"
check "nginx runs A's image" runs_image "$NGINX" "${IMG[a.nginx]}"
if [ "$pod_era" = true ]; then
  check "the pod-era release runs in podman-compose's pod" test -n "$(podman inspect --format '{{.Pod}}' "$ORCH")"
fi
check "the session aliases point at A's images" aliases_on a
check "the notification hook heard 'applied'" notified applied
check "no pre-deploy backup on a first install" test "$(backups)" -eq 0
check "nothing holds the lock" lock_free
md status
check "status names the installed commit" said "current:         ${commit:0:12}"
md unpin
check "unpin exits 0" exit_is 0
podman unshare sh -c "echo transitions > '${MARS_ROOT}/data/transitions-marker'"
id_before=$(identity)
pg_before=$(cid "$PG")

section "2. nothing new: no-op"
orch_before=$(cid "$ORCH")
md run
check "run exits 0" exit_is 0
check "it says up to date" said "up to date"
check "the orchestrator was not touched" test "$(cid "$ORCH")" = "$orch_before"
check "the last check is 'unchanged'" test "$(st .last_check.outcome)" = unchanged

section "3. B promoted: A -> B"
nginx_before=$(cid "$NGINX")
promote b
md run
check "run exits 0" exit_is 0
check "current links to release B" installed b
check "previous is A" test "$(st .previous.bundle)" = "${DIGEST[a]}"
check "the orchestrator was replaced by B's image" runs_image "$ORCH" "${IMG[b.orchestrator]}"
check "nginx was recreated on B's image" runs_image "$NGINX" "${IMG[b.nginx]}"
check "nginx is a new container" test "$(cid "$NGINX")" != "$nginx_before"
check "the orchestrator is in no pod" test -z "$(podman inspect --format '{{.Pod}}' "$ORCH")"
check "nginx is in no pod" test -z "$(podman inspect --format '{{.Pod}}' "$NGINX")"
check "PostgreSQL is the same container" test "$(cid "$PG")" = "$pg_before"
check "the database is the same cluster and rows" test "$(identity)" = "$id_before"
check "the data directory is the same" podman unshare test -f "${MARS_ROOT}/data/transitions-marker"
check "a pre-deploy backup was taken" test "$(backups)" -eq 1
check "the API routes through nginx to the new orchestrator" healthy
check "the session aliases moved to B's images" aliases_on b
check "nothing holds the lock" lock_free

section "4. A promoted again (a late, older run): ignored"
orch_before=$(cid "$ORCH")
promote a
md run
check "run exits 0" exit_is 0
check "it says not newer" said "not newer than the installed release"
check "the orchestrator was not touched" test "$(cid "$ORCH")" = "$orch_before"
check "B stays installed" installed b

section "5. rollback pins A; the pin holds against a newer main; unpin follows it again"
promote b
md rollback
check "rollback exits 0" exit_is 0
check "A is installed" installed a
check "the orchestrator runs A's image" runs_image "$ORCH" "${IMG[a.orchestrator]}"
check "the session aliases are back on A's images" aliases_on a
check "health through nginx is green" healthy
orch_before=$(cid "$ORCH")
md run
check "run while pinned exits 0" exit_is 0
check "it keeps the pin, not the promoted B" installed a
check "the orchestrator was not touched" test "$(cid "$ORCH")" = "$orch_before"
md unpin
md run
check "run after unpin exits 0" exit_is 0
check "B is installed again" installed b
check "the orchestrator runs B's image" runs_image "$ORCH" "${IMG[b.orchestrator]}"

section "6. paused: nothing resolved"
md pause
promote c
orch_before=$(cid "$ORCH")
md run
check "run exits 0" exit_is 0
check "it says paused" said "paused"
check "the orchestrator was not touched" test "$(cid "$ORCH")" = "$orch_before"
check "B stays installed" installed b
md resume

section "7. two runs at once: the second waits for the lock"
"${MARS_ROOT}/current/bin/mars-deploy" run >"${work}/log/first" 2>&1 &
first=$!
for _ in $(seq 1 100); do flock -n "$LOCK" true || break; sleep 0.1; done
check "the first run holds the lock" not flock -n "$LOCK" true
"${MARS_ROOT}/current/bin/mars-deploy" run >"${work}/log/second" 2>&1 &
second=$!
wait "$first"
first_rc=$?
wait "$second"
second_rc=$?
check "the first run applied C" grep -q "applied" "${work}/log/first"
check "the first run exits 0" test "$first_rc" -eq 0
check "the second run waited and found C installed" grep -q "up to date" "${work}/log/second"
check "the second run exits 0" test "$second_rc" -eq 0
check "C is installed" installed c
check "nothing holds the lock" lock_free

section "8. an image that cannot be pulled: deferred, nothing touched"
promote x
orch_before=$(cid "$ORCH")
md run
check "run exits 75" exit_is 75
check "the attempt is deferred" test "$(st .attempted.outcome)" = deferred
check "the release is not recorded as failed" not failed_has x 
check "the orchestrator was not touched" test "$(cid "$ORCH")" = "$orch_before"
check "C stays installed" installed c

section "9. the pre-deploy backup fails: deferred, nothing touched; fixed, it applies"
promote d
echo "MARS_BACKUP_AGE_RECIPIENTS=${work}/no-such-recipients" >>"$ENV_FILE"
md run
check "run exits 75" exit_is 75
check "the reason is the backup" test "$(st .attempted.reason)" = "the pre-deploy backup failed"
check "the orchestrator was not touched" test "$(cid "$ORCH")" = "$orch_before"
check "C stays installed" installed c
check "the session aliases were not moved" aliases_on c
sed -i '/^MARS_BACKUP_AGE_RECIPIENTS=/d' "$ENV_FILE"
md run
check "run after the fix exits 0" exit_is 0
check "D is installed" installed d

section "10. never healthy, no migration added: failed, D started again, then held"
promote f
: >"${work}/notified"
md run
check "run exits 1" exit_is 1
check "F is recorded as failed" failed_has f
check "the recovery was the automatic rollback" test "$(st .attempted.recovery)" = "rolled back to the previous release"
check "D stays installed" installed d
check "the orchestrator runs D's image again" runs_image "$ORCH" "${IMG[d.orchestrator]}"
check "the session aliases are back on D's images" aliases_on d
check "health through nginx is green" healthy
check "the notification hook heard 'failed'" notified failed
orch_before=$(cid "$ORCH")
md run
check "the next run exits 0" exit_is 0
check "it holds the known failure" said "failed before; held"
check "the orchestrator was not touched" test "$(cid "$ORCH")" = "$orch_before"

section "11. a higher epoch: held until accept-epoch"
promote e
md run
check "run exits 0" exit_is 0
check "it is held with the epoch and its note" said "epoch 2 needs operator action: Transitions test: nothing to do."
check "the orchestrator was not touched" test "$(cid "$ORCH")" = "$orch_before"
check "the notification hook heard 'held'" notified held
md accept-epoch 2
md run
check "run after accept-epoch exits 0" exit_is 0
check "E is installed" installed e

# held <bundle> <expected reason substring> <check name>
held_case() {
  promote "$1"
  orch_before=$(cid "$ORCH")
  md run
  check "$3: run exits 0" exit_is 0
  check "$3: held" test "$(st .attempted.outcome)" = held
  check "$3: the reason says why" said "$2"
  check "$3: the orchestrator was not touched" test "$(cid "$ORCH")" = "$orch_before"
  check "$3: E stays installed" installed e
}
section "12. the other held rules"
held_case g "configuration: MARS_TRANSITIONS_REQUIRED" "a missing required variable"
held_case h "do not extend the installed ones" "migrations that do not extend the installed ones"
held_case i "PostgreSQL 18 is running; the release is tested on 17" "another PostgreSQL major"
held_case j "the manifest does not validate" "another platform"

section "13. killed in the middle of the replacement: the next run recovers"
promote k
setsid "${MARS_ROOT}/current/bin/mars-deploy" run >"${work}/log/killed" 2>&1 &
victim=$!
for _ in $(seq 1 600); do
  [ "$(st '.attempted.phase' 2>/dev/null)" = replacing ] && [ "$(st '.attempted.target')" = "${DIGEST[k]}" ] && break
  sleep 0.1
done
check "the run reached the replacement" test "$(st .attempted.phase)" = replacing
kill -KILL -- "-${victim}" 2>/dev/null
wait "$victim" 2>/dev/null
md run
check "the next run exits 0" exit_is 0
check "K is recorded as failed" failed_has k
check "the reason is the interruption" test "$(st .attempted.reason)" = "interrupted during replacement"
check "the recovery was the automatic rollback" test "$(st .attempted.recovery)" = "rolled back to the previous release"
check "E stays installed" installed e
check "the orchestrator runs E's (D's) image" runs_image "$ORCH" "${IMG[d.orchestrator]}"
check "the session aliases are back on D's images" aliases_on d
check "health through nginx is green" healthy
check "nothing holds the lock" lock_free

section "14. stop and start: the boot path, from local images"
md stop
check "stop exits 0" exit_is 0
check "nginx is stopped" not running "$NGINX"
check "the orchestrator is stopped" not running "$ORCH"
check "PostgreSQL is stopped" not running "$PG"
md start
check "start exits 0" exit_is 0
check "health through nginx is green" healthy
check "the database is the same cluster and rows" test "$(identity)" = "$id_before"
check "nothing holds the lock" lock_free

stable=e
if [ "$units" = true ]; then
  stable=n
  failures_before_units=$failures
  unit_ok() { [ "$(systemctl --user show -p Result --value "$1")" = success ]; }
  section "14u. the user units (README.md, \"The user units\")"
  md stop
  n=$((n + 1))
  "${MARS_ROOT}/current/bin/install-units" --project "$PROJECT" \
    --env "MARS_REGISTRY=${MARS_REGISTRY}" --env "MARS_HEALTH_TIMEOUT=${MARS_HEALTH_TIMEOUT}" \
    --env "SESSION_IMAGES_BASE_ALIAS=${SESSION_IMAGES_BASE_ALIAS}" \
    --env "SESSION_IMAGES_DEV_ALIAS=${SESSION_IMAGES_DEV_ALIAS}" \
    --env "CONTAINERS_REGISTRIES_CONF=${CONTAINERS_REGISTRIES_CONF}" >"${work}/log/${n}" 2>&1
  rc=$?
  units_installed=true
  check "install-units exits 0" exit_is 0
  check "mars.service is enabled" test "$(systemctl --user is-enabled mars.service)" = enabled
  check "mars-backup.timer is enabled" test "$(systemctl --user is-enabled mars-backup.timer)" = enabled
  check "mars-deploy.timer is not" test "$(systemctl --user is-enabled mars-deploy.timer)" = disabled
  check "mars.service starts the installed release (boot)" systemctl --user start mars.service
  check "health through nginx is green" healthy
  check "mars.service stops it" systemctl --user stop mars.service
  check "the orchestrator is stopped" not running "$ORCH"
  check "PostgreSQL is stopped" not running "$PG"
  check "mars.service starts it again" systemctl --user start mars.service
  check "health through nginx is green" healthy

  promote n
  check "mars-deploy.service applies a promoted release" systemctl --user start mars-deploy.service
  check "N is installed" installed n
  check "the unit has finished" test "$(systemctl --user show -p ActiveState --value mars-deploy.service)" = inactive
  check "the orchestrator outlived the one-shot unit" running "$ORCH"
  check "nginx outlived the one-shot unit" running "$NGINX"
  check "health through nginx is green after the unit" healthy
  check "nothing holds the lock" lock_free

  check "mars-backup.service takes a full backup" systemctl --user start mars-backup.service
  check "the full set exists" test -n "$(find "${MARS_ROOT}/backups" -mindepth 1 -maxdepth 1 -name '*-full-nightly')"
  check "the orchestrator is running again after it" running "$ORCH"
  # mars-backup starts the orchestrator and nginx again and returns; it does
  # not wait for health, so neither does the unit.
  check "health through nginx is green after the backup" eventually 120 healthy

  promote p
  : >"${work}/notified"
  unit_start() { systemctl --user start "$1" 2>/dev/null; }
  check "mars-deploy.service fails on a release that never becomes healthy" not unit_start mars-deploy.service
  check "the unit's result is a failure" not unit_ok mars-deploy.service
  for _ in $(seq 1 100); do notified unit-failed && break; sleep 0.2; done
  check "mars-failed@ ran the notification hook" notified unit-failed
  check "N was started again" runs_image "$ORCH" "${IMG[n.orchestrator]}"
  check "health through nginx is green" healthy
  check "nothing holds the lock" lock_free
  if [ "$failures" -gt "$failures_before_units" ]; then
    journalctl --user -u mars.service -u mars-deploy.service -u mars-backup.service -u 'mars-failed@*' \
      -n 80 --no-pager 2>&1 | sed 's/^/     journal: /'
  fi
fi

section "15. never healthy after adding a migration: failed, left in place, manual recovery"
promote m
backups_before=$(backups)
md run
check "run exits 1" exit_is 1
check "M is recorded as failed" failed_has m
check "the recovery is manual" grep -q '"recovery": "manual: the release added migrations' "${MARS_ROOT}/state/state.json"
check "no rollback was attempted: the orchestrator is M's" runs_image "$ORCH" "${IMG[m.orchestrator]}"
check "current still names the last good release" installed "$stable"
check "the pre-deploy backup exists for the manual recovery" test "$(backups)" -eq $((backups_before + 1))
md status
check "status says manual recovery is needed" said "manual:"
orch_before=$(cid "$ORCH")
md run
check "the next run exits 0" exit_is 0
check "it holds the known failure" said "failed before; held"
check "the candidate's orchestrator was left as it is" test "$(cid "$ORCH")" = "$orch_before"
check "nothing holds the lock" lock_free

section "16. no value from mars.env anywhere the updater writes"
leaks=$(grep -rlF -e "$JWT" -e "$DBPW" -e "$MKEY" "${MARS_ROOT}/state" "${work}/log" "${work}/notified" 2>/dev/null)
check "the state, the output and the notifications carry no secret" test -z "$leaks"

echo
if [ "$failures" -gt 0 ]; then
  echo "${failures} check(s) failed"
  exit 1
fi
echo "all transition checks passed"
