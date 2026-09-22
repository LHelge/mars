#!/usr/bin/env bash
# Tests of the release scripts (ARCHITECTURE.md, "Server deployment"): the
# promotion decision over a scratch repository with a successor, a late
# older run and a rewritten main; the bundle assembled from HEAD with fake
# digests; and the manifest validator over the example and over broken
# copies of it. Needs git, jq and sha256sum; run from anywhere in the clone.
#
#   scripts/release/test.sh
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(git -C "$here" rev-parse --show-toplevel)
work=$(mktemp -d)
example="${root}/deploy/manifest.example.json"
trap 'rm -rf "$work"' EXIT

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
# expect <name> <expected stdout> <expected status> <command...>
expect() {
  local name=$1 want_out=$2 want_status=$3 out status
  shift 3
  set +e
  out=$("$@" 2>/dev/null)
  status=$?
  set -e
  if [ "$out" = "$want_out" ] && [ "$status" -eq "$want_status" ]; then
    pass "$name"
  else
    fail "$name: got '${out}' (exit ${status}), want '${want_out}' (exit ${want_status})"
  fi
}

# --- promote-decision.sh ------------------------------------------------
repo="${work}/repo"
git init -q -b main "$repo"
commit() {
  git -C "$repo" -c user.name=t -c user.email=t@example.invalid \
    commit -q --allow-empty -m "$1"
  git -C "$repo" rev-parse HEAD
}
c1=$(commit one)
c2=$(commit two)
c3=$(commit three)
git -C "$repo" checkout -q -b rewritten "$c1"
x2=$(commit rewritten-two)
git -C "$repo" checkout -q main

decide() { (cd "$repo" && "${here}/promote-decision.sh" "$@"); }
expect "nothing promoted yet promotes" promote 0 decide "" "$c1"
expect "a successor promotes" promote 0 decide "$c1" "$c3"
expect "a re-run of the promoted commit is same" same 0 decide "$c2" "$c2"
expect "a late run of an older commit is older" older 0 decide "$c3" "$c2"
expect "a diverged candidate fails" "" 1 decide "$c2" "$x2"
expect "a promoted commit missing from the clone fails" "" 1 decide "$(printf 'e%.0s' {1..40})" "$c3"
expect "a short commit id is refused" "" 2 decide "" "${c1:0:12}"

# --- make-bundle.sh -----------------------------------------------------
fake() { printf 'ghcr.io/lhelge/%s@sha256:%s' "$1" "$(printf '%s' "$1" | sha256sum | cut -c1-64)"; }
head=$(git -C "$root" rev-parse HEAD)
build_bundle() (
  cd "$root"
  RELEASE_COMMIT=$head RELEASE_RUN_ID=1 RELEASE_RUN_ATTEMPT=1 \
    IMAGE_ORCHESTRATOR=$(fake mars-orchestrator) IMAGE_NGINX=$(fake mars-nginx) \
    IMAGE_SESSION_CLAUDE=$(fake mars-session-claude) \
    IMAGE_SESSION_CLAUDE_DEV=$(fake mars-session-claude-dev) \
    "${here}/make-bundle.sh" "${work}/out" >/dev/null
)
check "the bundle for HEAD assembles and validates" build_bundle

manifest="${work}/out/bundle/manifest.json"
if [ -f "$manifest" ]; then
  want=$(git -C "$root" ls-tree --name-only "${head}:orchestrator/migrations" | grep -c '\.up\.sql$')
  got=$(jq '.schema.migrations | length' "$manifest")
  check "every migration is listed (${got} of ${want})" test "$got" = "$want"
  check "the manifest names HEAD" test "$(jq -r .source.commit "$manifest")" = "$head"
  check "the sequence is the commit count" \
    test "$(jq .source.sequence "$manifest")" = "$(git -C "$root" rev-list --count "$head")"
  check "compose.release.yml pins the orchestrator digest" \
    grep -q "image: $(fake mars-orchestrator)" "${work}/out/bundle/compose.release.yml"
  for f in compose.yml compose.podman.yml scripts/verify-deployment.sh bin/check-env bin/session-images env.example; do
    check "the bundle carries ${f}" test -f "${work}/out/bundle/${f}"
  done
fi

# The release override renders over the two files it extends. podman-compose
# is the implementation the server runs; the check is skipped without it.
if command -v podman-compose >/dev/null 2>&1 && [ -f "$manifest" ]; then
  cp "${root}/.env.example" "${work}/out/bundle/.env"
  if rendered=$(cd "${work}/out/bundle" &&
    podman-compose -f compose.yml -f compose.podman.yml -f compose.release.yml config 2>/dev/null) &&
    grep -q "image: $(fake mars-orchestrator)" <<<"$rendered" &&
    grep -q "image: $(fake mars-nginx)" <<<"$rendered" &&
    grep -q 'userns_mode: keep-id' <<<"$rendered"; then
    pass "podman-compose renders the release with its digests and keep-id"
  else
    fail "podman-compose renders the release with its digests and keep-id"
  fi
else
  echo "skip podman-compose render (podman-compose not installed)"
fi
expect "an existing output directory is refused" "" 2 \
  env RELEASE_COMMIT="$head" "${here}/make-bundle.sh" "${work}/out"
expect "a tag instead of a digest is refused" "" 1 bash -c "cd '$root' &&
  RELEASE_COMMIT=$head RELEASE_RUN_ID=1 RELEASE_RUN_ATTEMPT=1 \
  IMAGE_ORCHESTRATOR=ghcr.io/lhelge/mars-orchestrator:latest IMAGE_NGINX=$(fake mars-nginx) \
  IMAGE_SESSION_CLAUDE=$(fake mars-session-claude) IMAGE_SESSION_CLAUDE_DEV=$(fake mars-session-claude-dev) \
  '${here}/make-bundle.sh' '${work}/tagged' >/dev/null"

# --- deploy/bin/check-env ----------------------------------------------
checkenv="${root}/deploy/bin/check-env"
mkdir -p "${work}/env/data"
python3 -c 'import socket,sys; socket.socket(socket.AF_UNIX).bind(sys.argv[1])' "${work}/env/engine.sock"
good_env() {
  cat <<ENV
# a comment, then the variables a release requires
PUBLIC_URL=https://mars.example.invalid
JWT_SECRET="fake-jwt-secret-0123456789abcdefghijklmnop"
POSTGRES_USER=mars
POSTGRES_PASSWORD=fake-db-password_1.2~3
POSTGRES_DB=mars
POSTGRES_IMAGE=docker.io/library/postgres@sha256:$(printf '0%.0s' {1..64})
ENGINE_SOCKET_HOST=${work}/env/engine.sock
DATA_DIR_HOST=${work}/env/data
SECRETS_MASTER_KEYS=1=ZmFrZS1tYXN0ZXIta2V5LWZvci10ZXN0cy1vbmx5LTEyMzQ=
GIT_BOT_NAME='Mars Bot'
GIT_BOT_EMAIL=mars-bot@example.invalid
ENV
}
env_case() { # env_case <name> <want status> <sed expression or "">
  good_env >"${work}/env/mars.env"
  if [ -n "$3" ]; then sed -i "$3" "${work}/env/mars.env"; fi
  chmod 600 "${work}/env/mars.env"
  expect "check-env: $1" "" "$2" bash -c "'$checkenv' '${work}/env/mars.env' '$example' >/dev/null"
}
env_case "a complete private file passes" 0 ""
env_case "a missing required variable fails" 1 "/^GIT_BOT_EMAIL=/d"
env_case "either master key variable satisfies the pair" 0 "s#^SECRETS_MASTER_KEYS=.*#SECRETS_MASTER_KEY_FILE=/run/secrets/fake#"
env_case "both master key variables fail" 1 "\$a SECRETS_MASTER_KEY_FILE=/run/secrets/fake"
env_case "the example placeholder password fails" 1 "s#^POSTGRES_PASSWORD=.*#POSTGRES_PASSWORD=change-me-local-password#"
env_case "a password that breaks the URL fails" 1 "s#^POSTGRES_PASSWORD=.*#POSTGRES_PASSWORD=fake@pass/word#"
env_case "a relative data directory fails" 1 "s#^DATA_DIR_HOST=.*#DATA_DIR_HOST=./data#"
env_case "an engine socket that is not a socket fails" 1 "s#^ENGINE_SOCKET_HOST=.*#ENGINE_SOCKET_HOST=${work}/env/data#"
env_case "a line that is not KEY=VALUE fails" 1 "\$a export BROKEN"
env_case "an unpinned postgres image only warns" 0 "s#^POSTGRES_IMAGE=.*#POSTGRES_IMAGE=postgres:18#"
good_env >"${work}/env/mars.env"
chmod 644 "${work}/env/mars.env"
expect "check-env: a world-readable file fails" "" 1 bash -c "'$checkenv' '${work}/env/mars.env' '$example' >/dev/null"
chmod 600 "${work}/env/mars.env"
leak=$("$checkenv" "${work}/env/mars.env" "$example" 2>&1 || true)
# shellcheck disable=SC2016 # $1 is expanded by the inner bash, not here
check "check-env never prints a value" bash -c '! grep -qE "fake-jwt|fake-db|ZmFrZS" <<<"$1"' _ "$leak"

# --- deploy/bin/session-images ------------------------------------------
# The engine behaviour the managed aliases rely on (ARCHITECTURE.md, "Server
# deployment", "Session images"), on throwaway images and aliases under a
# test prefix: a moved alias is what a new container gets, an existing
# container keeps its image, an image named by anything else is untouched,
# and moving the alias back is the rollback.
if command -v podman >/dev/null 2>&1; then
  tag="localhost/mars-release-test-$$"
  export SESSION_IMAGES_BASE_ALIAS="${tag}-alias-base:latest" SESSION_IMAGES_DEV_ALIAS="${tag}-alias-dev:latest"
  si="${root}/deploy/bin/session-images"
  created=()
  cleanup_images() {
    podman rm -f "${created[@]}" >/dev/null 2>&1 || true
    podman rmi -f "${tag}-v1" "${tag}-v2" "${tag}-custom" \
      "$SESSION_IMAGES_BASE_ALIAS" "$SESSION_IMAGES_DEV_ALIAS" >/dev/null 2>&1 || true
  }
  mkimg() { # <name> <content>
    mkdir -p "${work}/img-$1"
    printf '%s\n' "$2" >"${work}/img-$1/version"
    printf 'FROM scratch\nCOPY version /version\n' >"${work}/img-$1/Containerfile"
    podman build -q -t "${tag}-$1" "${work}/img-$1" >/dev/null
  }
  id_of() { podman image inspect --format '{{.Id}}' "$1"; }
  ctr_image() { podman inspect --format '{{.Image}}' "$1"; }
  new_ctr() { # a container created from a name, as the launcher would
    local c
    c=$(podman create --entrypoint /none "$1")
    created+=("$c")
    echo "$c"
  }
  mkimg v1 one && mkimg v2 two && mkimg custom custom
  v1=$(id_of "${tag}-v1")
  v2=$(id_of "${tag}-v2")
  custom=$(id_of "${tag}-custom")

  expect "session-images: absent aliases read as -" "${SESSION_IMAGES_BASE_ALIAS} -" 0 \
    bash -c "'$si' get | head -1"
  expect "session-images: an absent image is refused and nothing moves" "" 1 \
    "$si" set "${tag}-v1" "${tag}-missing"
  check "session-images: the refused set left the dev alias absent" \
    bash -c "! podman image exists '$SESSION_IMAGES_DEV_ALIAS'"

  "$si" set "${tag}-v1" "${tag}-v1" >/dev/null
  running=$(new_ctr "$SESSION_IMAGES_DEV_ALIAS")
  pinned=$(new_ctr "${tag}-custom")
  check "session-images: release A launches on A" test "$(ctr_image "$running")" = "$v1"

  "$si" set "${tag}-v2" "${tag}-v2" >/dev/null
  check "session-images: after the switch a new launch gets B" \
    test "$(ctr_image "$(new_ctr "$SESSION_IMAGES_DEV_ALIAS")")" = "$v2"
  check "session-images: a container from A keeps A" test "$(ctr_image "$running")" = "$v1"
  check "session-images: a custom image is untouched" \
    test "$(id_of "${tag}-custom")" = "$custom"
  check "session-images: its container keeps it" test "$(ctr_image "$pinned")" = "$custom"
  check "session-images: get reports B" \
    bash -c "'$si' get | grep -q '^${SESSION_IMAGES_DEV_ALIAS} ${v2}\$'"

  "$si" set "${tag}-v1" "${tag}-v1" >/dev/null
  check "session-images: rolling back makes new launches A again" \
    test "$(ctr_image "$(new_ctr "$SESSION_IMAGES_DEV_ALIAS")")" = "$v1"
  cleanup_images
  unset SESSION_IMAGES_BASE_ALIAS SESSION_IMAGES_DEV_ALIAS
else
  echo "skip session-images (podman not installed)"
fi

# --- validate-manifest.sh -----------------------------------------------
expect "the example manifest is valid" "" 0 "${here}/validate-manifest.sh" "$example"
broken() {
  jq "$2" "$example" >"${work}/broken.json"
  expect "refused: $1" "" 1 "${here}/validate-manifest.sh" "${work}/broken.json"
}
broken "unknown format" '.format = 2'
broken "missing epoch" 'del(.epoch)'
broken "fractional epoch" '.epoch = 1.5'
broken "short commit" '.source.commit = "abc123"'
broken "image by tag" '.images.nginx = "ghcr.io/lhelge/mars-nginx:main"'
broken "image from another registry" '.images.orchestrator = "docker.io/lhelge/mars-orchestrator@sha256:" + ("a" * 64)'
broken "shell in a variable name" '.config.required += ["X;rm -rf /"]'
broken "unsorted migrations" '.schema.migrations |= reverse'
broken "wrong platform" '.platform = "linux/arm64"'
cp "$example" "${work}/extra.json"
jq '.future_field = {"anything": true}' "$example" >"${work}/extra.json"
expect "an unknown field is ignored" "" 0 "${here}/validate-manifest.sh" "${work}/extra.json"
echo 'not json' >"${work}/garbage.json"
expect "refused: not JSON" "" 1 "${here}/validate-manifest.sh" "${work}/garbage.json"

if [ "$failures" -ne 0 ]; then
  echo "${failures} failure(s)"
  exit 1
fi
echo "all release script tests passed"
