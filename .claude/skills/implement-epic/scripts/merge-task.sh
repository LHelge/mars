#!/usr/bin/env bash
# Merge a subagent's task branch onto main and verify it.
#
#   merge-task.sh <branch> [--task <bears id>] [--after-conflict] [--no-verify]
#
# Cherry-picks every commit in main..<branch> onto main (the branch base is
# often behind main, so a fast-forward is rarely possible), removes the
# branch's worktree and the branch, runs the quality chains for the areas the
# commits touched, and on success marks the Bears task done with `bea`.
#
# The backend chain builds into orchestrator/target-main, the coordinator's own
# target directory that no subagent worktree ever builds into: nothing in it
# can be a sibling's stale artifact, so the chain runs incrementally and no
# source touching is needed. Subagents keep sharing orchestrator/target.
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
  # The harness locks the worktree of an agent it spawned and may keep the lock
  # after the agent has reported; `remove` refuses a locked tree even with
  # --force, so unlock first.
  git worktree unlock "$WT" 2>/dev/null || true
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
    TARGET_DIR="$REPO/orchestrator/target-main"
    echo "== backend chain (CARGO_TARGET_DIR=$TARGET_DIR, DOCKER_HOST=${DOCKER_HOST:-unset})"
    # Two full target directories live here; a link step that runs out of room
    # fails late and confusingly, so say it before the chain instead.
    FREE_GB="$(df -Pk "$REPO" | awk 'NR==2 {print int($4 / 1048576)}')"
    if [ -n "$FREE_GB" ] && [ "$FREE_GB" -lt 25 ]; then
      echo "!! only ${FREE_GB} GB free on $REPO; target-main needs about the size of" >&2
      echo "   orchestrator/target (18 GB today) and a link step may fail mid-way" >&2
    fi
    mkdir -p "$TARGET_DIR"
    CHAIN_LOG="$TARGET_DIR/merge-chain.log"
    : > "$CHAIN_LOG"
    (
      cd orchestrator
      # target-main is the coordinator's alone: no worktree builds into it, so
      # cargo cannot reuse a sibling's artifact here and the chain is
      # incremental. Never point this at orchestrator/target.
      export CARGO_TARGET_DIR="$TARGET_DIR"
      # `sqlx::migrate!` embeds the SQL at compile time and cargo does not
      # notice a changed `.sql` file, so the Migrator's users are touched.
      if echo "$CHANGED" | grep -q '^orchestrator/migrations/'; then
        touch tests/common/db.rs tests/migrations.rs src/main.rs
      fi
      # fmt is independent of the target directory, and cheapest, so it is first.
      cargo fmt --check
      cargo clippy --all-targets -- -D warnings 2>&1 | tee -a "$CHAIN_LOG"
      cargo clippy --all-targets --features integration-tests -- -D warnings 2>&1 | tee -a "$CHAIN_LOG"
      if [ -n "${DOCKER_HOST:-}" ]; then
        cargo test --features integration-tests 2>&1 | tee -a "$CHAIN_LOG"
      else
        echo "!! no container engine: skipping tests named health_*; CI runs them"
        cargo test --features integration-tests -- --skip health_ 2>&1 | tee -a "$CHAIN_LOG"
      fi
    )
    # If the merged commits changed the crate's sources, the chain must have
    # compiled them. A "clean" chain that compiled nothing verified nothing.
    if echo "$CHANGED" | grep -qE '^orchestrator/(src|tests|migrations)/'; then
      if ! grep -q 'Compiling mars-orchestrator' "$CHAIN_LOG"; then
        echo "!! the chain never printed 'Compiling mars-orchestrator' although $BRANCH" >&2
        echo "   changed orchestrator sources, so it verified a stale build." >&2
        echo "   Output: $CHAIN_LOG. Check that nothing else builds into $TARGET_DIR," >&2
        echo "   then delete it and rerun the chain by hand. (A chain you already ran" >&2
        echo "   by hand on this same commit leaves nothing to compile and trips this too.)" >&2
        exit 1
      fi
      echo "== chain compiled mars-orchestrator (log: $CHAIN_LOG)"
    fi
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
    # action-validator checks the schema only; it accepted a job-level `env`
    # that used the `runner` context, which GitHub then refused to parse, so
    # the Images run failed before any job started. actionlint knows which
    # contexts each key may use. It needs the repository root as its working
    # directory and a container engine to run from.
    ENGINE_CLI=""
    if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then ENGINE_CLI=docker
    elif command -v podman >/dev/null 2>&1; then ENGINE_CLI=podman
    fi
    if [ -n "$ENGINE_CLI" ]; then
      "$ENGINE_CLI" run --rm -v "$REPO:/repo:ro" -w /repo docker.io/rhysd/actionlint:latest -no-color
    else
      echo "!! no container engine for actionlint; expression-context errors are only caught by GitHub"
    fi
  fi
fi

if [ -n "$TASK" ]; then
  bea done "$TASK" --json >/dev/null && echo "== bears: $TASK done"
fi

echo "== ok; next: review the next report or dispatch the next wave (push once when the epic closes)"
