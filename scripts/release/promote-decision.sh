#!/usr/bin/env bash
# Decide whether a candidate commit may move the promoted `mars-deploy:main`
# tag (ARCHITECTURE.md, "Server deployment", "Promotion"). Run inside a full
# clone of the repository.
#
#   scripts/release/promote-decision.sh <promoted-commit or ""> <candidate-commit>
#
# Prints one word and exits 0:
#   promote  nothing is promoted yet, or the candidate strictly descends from it
#   same     the candidate is the promoted commit (a re-run); nothing to do
#   older    the candidate is an ancestor of the promoted commit (a late run)
# Exits 1 when the two have diverged (main was rewritten), 2 on bad input.
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <promoted-commit or \"\"> <candidate-commit>" >&2
  exit 2
fi
promoted=$1
candidate=$2

for commit in "$promoted" "$candidate"; do
  if [ -n "$commit" ] && ! [[ "$commit" =~ ^[0-9a-f]{40}$ ]]; then
    echo "not a full commit id: ${commit}" >&2
    exit 2
  fi
done
if [ -z "$candidate" ]; then
  echo "the candidate commit is required" >&2
  exit 2
fi
if ! git cat-file -e "${candidate}^{commit}" 2>/dev/null; then
  echo "candidate ${candidate} is not in this clone" >&2
  exit 2
fi

if [ -z "$promoted" ]; then
  echo promote
elif [ "$promoted" = "$candidate" ]; then
  echo same
elif ! git cat-file -e "${promoted}^{commit}" 2>/dev/null; then
  # The promoted commit is not in main's history at all: main was rewritten.
  echo "promoted ${promoted} is not in this clone; main has diverged from the promoted release" >&2
  exit 1
elif git merge-base --is-ancestor "$promoted" "$candidate"; then
  echo promote
elif git merge-base --is-ancestor "$candidate" "$promoted"; then
  echo older
else
  echo "candidate ${candidate} and promoted ${promoted} have diverged" >&2
  exit 1
fi
