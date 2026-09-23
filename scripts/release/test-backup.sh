#!/usr/bin/env bash
# Tests of deploy/bin/mars-backup (README.md, "Backups and recovery") against
# a real postgres:18 and a stand-in orchestrator on Podman, under a project
# name of their own so a developer's stack is never touched. Covers the db
# and full sets, their metadata, restore from them, the orchestrator being
# started again even when the backup fails, retention, the hook, and — when
# `age` is on PATH — encryption. Needs podman and jq.
#
#   scripts/release/test-backup.sh
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(git -C "$here" rev-parse --show-toplevel)
backup="${root}/deploy/bin/mars-backup"
work=$(mktemp -d)
project="bkt$$"
pg="${project}_postgres_1"
orch="${project}_orchestrator_1"
restore="${project}_restore"

failures=0
pass() { echo "ok   $1"; }
fail() {
  echo "FAIL $1"
  failures=$((failures + 1))
}
fails() { ! "$@" >/dev/null 2>&1; }
lacks() { ! grep -q "$1" <<<"$2"; }
restores() { podman exec -i "$1" pg_restore --list <"$2" >/dev/null; }
plaintext_files() { find "$1" -type f ! -name metadata.json ! -name '*.age' | wc -l; }
check() {
  local name=$1
  shift
  if "$@"; then pass "$name"; else fail "$name"; fi
}
cleanup() {
  podman rm -f -t 0 "$pg" "$orch" "$restore" >/dev/null 2>&1 || true
  podman unshare rm -rf "$work" 2>/dev/null || rm -rf "$work"
}
trap cleanup EXIT

sql() { podman exec "$1" psql -At -U mars -d mars -c "$2"; }
# pg_isready alone is not enough on a first start: the image initialises
# with a temporary server, which pg_isready already reports as ready before
# the database exists, and then restarts. Its log line marks the end of that
# phase; a real query against the database then proves the final server.
wait_pg() {
  for _ in $(seq 1 90); do
    if podman logs "$1" 2>&1 | grep -q 'PostgreSQL init process complete' &&
      podman exec "$1" psql -At -U mars -d mars -c 'SELECT 1' >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  echo "postgres in $1 never became ready" >&2
  return 1
}

podman run -d --name "$pg" -e POSTGRES_USER=mars -e POSTGRES_PASSWORD=fake-test-password \
  -e POSTGRES_DB=mars docker.io/library/postgres:18 >/dev/null
podman run -d --name "$orch" docker.io/library/busybox:1 sleep 3600 >/dev/null
wait_pg "$pg"
sql "$pg" "CREATE TABLE _sqlx_migrations (version BIGINT PRIMARY KEY, success BOOLEAN NOT NULL);
  INSERT INTO _sqlx_migrations VALUES (20260917065232, true), (20260922081500, true);
  CREATE TABLE marker (note TEXT); INSERT INTO marker VALUES ('before the backup');" >/dev/null

mkdir -p "${work}/data/projects/p1/shared/target" "${work}/data/sessions/s1/work"
echo transcript >"${work}/data/sessions/s1/work/file"
echo cache >"${work}/data/projects/p1/shared/target/big"
cat >"${work}/mars.env" <<ENV
POSTGRES_USER=mars
POSTGRES_PASSWORD=fake-test-password
POSTGRES_DB=mars
DATA_DIR_HOST=${work}/data
SECRETS_MASTER_KEYS=1=ZmFrZS1rZXktZm9yLXRlc3RzLW9ubHktMDEyMzQ1Njc4OWFi
MARS_BACKUP_DIR=${work}/backups
MARS_BACKUP_KEEP_DB=2
MARS_BACKUP_HOOK=${work}/hook
ENV
chmod 600 "${work}/mars.env"
printf '#!/bin/sh\necho "$@" >>"%s/hook.log"\n' "$work" >"${work}/hook"
chmod +x "${work}/hook"
mb() { "$backup" --env "${work}/mars.env" --project "$project" "$@"; }
latest() { find "${work}/backups" -mindepth 1 -maxdepth 1 -type d -name "*$1*" | sort | tail -1; }

# --- db -------------------------------------------------------------------
out=$(mb db --label pre-deploy 2>&1)
set_db=$(latest db-pre-deploy)
check "db: a set is written" test -f "${set_db}/db.dump"
check "db: the metadata names both migrations" \
  test "$(jq -c .schema.migrations "${set_db}/metadata.json")" = '["20260917065232","20260922081500"]'
check "db: the checksum matches the file" \
  test "$(jq -r '.files["db.dump"].sha256' "${set_db}/metadata.json")" = "$(sha256sum "${set_db}/db.dump" | cut -d' ' -f1)"
check "db: the hook ran with the kind and the set" grep -q "^db ${set_db}\$" "${work}/hook.log"
check "db: no value from the environment file is printed" lacks fake-test-password "$out"

podman run -d --name "$restore" -e POSTGRES_USER=mars -e POSTGRES_PASSWORD=fake-test-password \
  -e POSTGRES_DB=mars docker.io/library/postgres:18 >/dev/null
wait_pg "$restore"
podman exec -i "$restore" pg_restore -U mars -d mars --no-owner <"${set_db}/db.dump"
check "db: the dump restores into a fresh server" \
  test "$(sql "$restore" "SELECT note FROM marker")" = "before the backup"
podman rm -f -t 0 "$restore" >/dev/null

# --- full -----------------------------------------------------------------
started_before=$(podman inspect --format '{{.State.StartedAt}}' "$orch")
mb full >/dev/null 2>&1
set_full=$(latest full)
check "full: the orchestrator is running again" \
  test "$(podman inspect --format '{{.State.Running}}' "$orch")" = true
check "full: it was stopped and started" \
  test "$(podman inspect --format '{{.State.StartedAt}}' "$orch")" != "$started_before"
listing=$(tar -tzf "${set_full}/data.tar.gz")
check "full: session data is archived" grep -q '^./sessions/s1/work/file$' <<<"$listing"
check "full: shared directories are left out" lacks shared/target/big "$listing"
check "full: unencrypted, the environment file stays out" test ! -e "${set_full}/mars.env"

# --- failures -------------------------------------------------------------
podman stop -t 0 "$pg" >/dev/null
check "a stopped database fails the backup" fails mb db
check "a failed backup leaves nothing behind" \
  test -z "$(find "${work}/backups" -maxdepth 1 -name '.partial-*')"
podman start "$pg" >/dev/null
wait_pg "$pg"

# --- retention and the hook ---------------------------------------------
sleep 1; mb db >/dev/null 2>&1
sleep 1; mb db >/dev/null 2>&1
check "retention keeps MARS_BACKUP_KEEP_DB db sets" \
  test "$(find "${work}/backups" -mindepth 1 -maxdepth 1 -type d -name '*Z-db*' | wc -l)" = 2
check "retention does not count full sets" test -d "$set_full"
printf '#!/bin/sh\nexit 7\n' >"${work}/hook"
sleep 1
set +e
mb db >/dev/null 2>&1
status=$?
set -e
check "a failing hook exits 3" test "$status" = 3
check "and keeps the set" test "$(find "${work}/backups" -mindepth 1 -maxdepth 1 -type d -name '*Z-db*' | wc -l)" = 2

# --- encryption -----------------------------------------------------------
if command -v age >/dev/null 2>&1 && command -v age-keygen >/dev/null 2>&1; then
  age-keygen -o "${work}/identity" 2>/dev/null
  age-keygen -y "${work}/identity" >"${work}/recipients"
  printf '#!/bin/sh\nexit 0\n' >"${work}/hook"
  echo "MARS_BACKUP_AGE_RECIPIENTS=${work}/recipients" >>"${work}/mars.env"
  sleep 1
  mb full >/dev/null 2>&1
  set_enc=$(latest full)
  check "age: every file is encrypted" \
    test "$(plaintext_files "$set_enc")" = 0
  check "age: the environment file is included, encrypted" test -f "${set_enc}/mars.env.age"
  check "age: the metadata says so" test "$(jq .encrypted "${set_enc}/metadata.json")" = true
  age -d -i "${work}/identity" "${set_enc}/db.dump.age" >"${work}/restored.dump"
  check "age: the dump decrypts and reads back" restores "$pg" "${work}/restored.dump"
  # A failure after the orchestrator was stopped: an unusable recipient.
  sed -i "s#^MARS_BACKUP_AGE_RECIPIENTS=.*#MARS_BACKUP_AGE_RECIPIENTS=${work}/hook#" "${work}/mars.env"
  sleep 1
  check "a backup failing mid-way fails" fails mb full
  check "and still starts the orchestrator again" \
    test "$(podman inspect --format '{{.State.Running}}' "$orch")" = true
else
  echo "skip encryption (age not installed)"
fi

check "list shows the sets" test "$(mb list | wc -l)" -ge 3

if [ "$failures" -ne 0 ]; then
  echo "${failures} failure(s)"
  exit 1
fi
echo "all backup tests passed"
