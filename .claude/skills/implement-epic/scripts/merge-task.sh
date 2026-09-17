#!/usr/bin/env bash
# Merge a subagent's task branch onto main and verify it.
#
#   merge-task.sh <branch> [--task <bears id>] [--after-conflict] [--no-verify]
#
# Cherry-picks every commit in main..<branch> onto main (the branch base is
# often behind main, so a fast-forward is rarely possible), removes the
# branch's worktree and the branch, forces a rebuild of the orchestrator crate
# when backend files changed (a shared CARGO_TARGET_DIR can hold a test binary
# compiled in a deleted worktree), runs the quality chains for the areas the
# commits touched, and on success marks the Bears task done with `bea`.
#
# On a cherry-pick conflict the script stops. Resolve the files, run
# `git cherry-pick --continue`, then rerun with `--after-conflict` to do the
# cleanup and verification for the commits that are now on main.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
BRANCH=""
TASK=""
AFTER_CONFLICT=0
VERIFY=1

while [ $# -gt 0 ]; do
  case "$1" in
    --task) TASK="$2"; shift 2 ;;
    --after-conflict) AFTER_CONFLICT=1; shift ;;
    --no-verify) VERIFY=0; shift ;;
    -*) echo "unknown option: $1" >&2; exit 2 ;;
    *) BRANCH="$1"; shift ;;
  esac
done
[ -n "$BRANCH" ] || { echo "usage: merge-task.sh <branch> [--task <id>] [--after-conflict] [--no-verify]" >&2; exit 2; }

cd "$REPO"
git remote get-url origin | grep -q 'LHelge/mars' || { echo "not the Mars repository: $REPO" >&2; exit 2; }
[ "$(git branch --show-current)" = "main" ] || { echo "main must be checked out" >&2; exit 2; }

BASE_FILE=".git/implement-epic-base"

if [ "$AFTER_CONFLICT" -eq 0 ]; then
  git rev-parse --verify --quiet "$BRANCH" >/dev/null || { echo "no such branch: $BRANCH" >&2; exit 2; }
  if [ -n "$(git status --porcelain | grep -v '^ M .bears/' | grep -v '^?? ' || true)" ]; then
    echo "working tree has changes outside .bears/; commit or stash them first" >&2
    git status --short >&2
    exit 2
  fi
  COMMITS="$(git rev-list --reverse "main..$BRANCH")"
  [ -n "$COMMITS" ] || { echo "nothing to merge: $BRANCH has no commits beyond main" >&2; exit 2; }
  git rev-parse HEAD > "$BASE_FILE"
  echo "== commits to cherry-pick"
  git log --oneline "main..$BRANCH"
  if ! git cherry-pick $COMMITS; then
    echo
    echo "== cherry-pick stopped on a conflict; resolve these files, then:" >&2
    git diff --name-only --diff-filter=U >&2
    echo "   git cherry-pick --continue" >&2
    echo "   $0 $BRANCH ${TASK:+--task $TASK }--after-conflict" >&2
    exit 1
  fi
else
  [ -f "$BASE_FILE" ] || { echo "no recorded base; run without --after-conflict first" >&2; exit 2; }
  git cherry-pick --quit 2>/dev/null || true
fi

BASE="$(cat "$BASE_FILE")"
rm -f "$BASE_FILE"
echo "== merged: $(git log --oneline "$BASE..HEAD" | wc -l) commit(s), main at $(git rev-parse --short HEAD)"

# Worktree and branch cleanup.
WT="$(git worktree list --porcelain | awk -v b="refs/heads/$BRANCH" '$1=="worktree"{wt=$2} $1=="branch" && $2==b {print wt}')"
if [ -n "$WT" ]; then
  git worktree remove --force "$WT" || true
fi
git worktree prune
git branch -D "$BRANCH" >/dev/null 2>&1 && echo "== deleted branch $BRANCH" || true

if [ "$VERIFY" -eq 1 ]; then
  CHANGED="$(git diff --name-only "$BASE..HEAD")"
  if [ -z "${DOCKER_HOST:-}" ] && [ -S "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/podman/podman.sock" ]; then
    export DOCKER_HOST="unix://${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/podman/podman.sock"
  fi
  if echo "$CHANGED" | grep -q '^orchestrator/'; then
    echo "== backend chain (DOCKER_HOST=${DOCKER_HOST:-unset})"
    (
      cd orchestrator
      cargo clean -p mars-orchestrator --quiet
      cargo fmt --check
      cargo clippy --all-targets -- -D warnings
      cargo clippy --all-targets --features integration-tests -- -D warnings
      if [ -n "${DOCKER_HOST:-}" ]; then
        cargo test --features integration-tests
      else
        echo "!! no container engine: skipping tests named health_*; CI runs them"
        cargo test --features integration-tests -- --skip health_
      fi
    )
  fi
  if echo "$CHANGED" | grep -q '^frontend/'; then
    echo "== frontend chain"
    (
      cd frontend
      if echo "$CHANGED" | grep -q '^frontend/package-lock.json'; then npm ci; fi
      npm run lint
      npx tsc -b
      npm run build
      npm run test:unit
      npm run test:e2e
    )
  fi
  if echo "$CHANGED" | grep -q '^\.github/workflows/'; then
    echo "== workflow validation"
    for f in $(echo "$CHANGED" | grep '^\.github/workflows/'); do
      [ -f "$f" ] && npx -y @action-validator/cli@latest "$f"
    done
  fi
fi

if [ -n "$TASK" ]; then
  bea done "$TASK" --json >/dev/null && echo "== bears: $TASK done"
fi

echo "== ok; next: git push origin main"
