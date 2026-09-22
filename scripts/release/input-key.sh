#!/usr/bin/env bash
# Print the build-input key of one release image: a hash of the git objects
# its build reads, so an image whose inputs did not change between two commits
# is reused rather than rebuilt (ARCHITECTURE.md, "Server deployment",
# "Artifacts"). The key covers the repository side only; a base image that
# moved upstream under an unchanged tag does not change it, so reuse keeps
# the base an image was first built on until its own inputs change.
#
#   scripts/release/input-key.sh <orchestrator|nginx|session-claude|session-claude-dev> [base-digest]
#
# session-claude-dev also takes the digest reference of the base it is built
# on, since the same Dockerfile over a new base is a different image.
set -euo pipefail

commit=${RELEASE_COMMIT:-HEAD}
tree() { git rev-parse "${commit}:$1"; }

case "${1:-}" in
orchestrator) inputs="orchestrator=$(tree orchestrator)" ;;
nginx) inputs="frontend=$(tree frontend) nginx=$(tree nginx) dockerignore=$(tree .dockerignore)" ;;
session-claude) inputs="claude=$(tree images/claude)" ;;
session-claude-dev)
  if [ -z "${2:-}" ]; then
    echo "session-claude-dev needs the base image digest reference" >&2
    exit 2
  fi
  inputs="claude-dev=$(tree images/claude-dev) base=$2"
  ;;
*)
  echo "usage: $0 <orchestrator|nginx|session-claude|session-claude-dev> [base-digest]" >&2
  exit 2
  ;;
esac

# Prefixed with the image name so two images can never share a key.
printf '%s %s\n' "$1" "$inputs" | sha256sum | cut -c1-40
