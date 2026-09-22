#!/usr/bin/env bash
# Validate a release manifest against ARCHITECTURE.md, "Server deployment",
# "The manifest": every required field present and matching its pattern.
# Unknown fields are ignored. The manifest is only ever read through jq here;
# nothing in it is sourced or evaluated.
#
#   scripts/release/validate-manifest.sh <manifest.json>
#
# Exits 0 when valid, 1 naming the first failing field otherwise.
set -euo pipefail

if [ "$#" -ne 1 ] || [ ! -f "$1" ]; then
  echo "usage: $0 <manifest.json>" >&2
  exit 2
fi

# Each check is `field<TAB>jq expression`; the expression must be true.
checks=$(cat <<'CHECKS'
format	.format == 1
epoch	(.epoch | type == "number") and .epoch >= 1 and (.epoch | floor) == .epoch
epoch_note	.epoch_note | type == "string"
source.repository	.source.repository == "LHelge/mars"
source.commit	.source.commit | type == "string" and test("^[0-9a-f]{40}$")
source.sequence	(.source.sequence | type == "number") and .source.sequence >= 1 and (.source.sequence | floor) == .source.sequence
source.run_id	.source.run_id | type == "number"
source.run_attempt	.source.run_attempt | type == "number"
source.published_at	.source.published_at | type == "string" and test("^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\\.[0-9]+)?(Z|[+-][0-9]{2}:[0-9]{2})$")
platform	.platform == "linux/amd64"
images.orchestrator	.images.orchestrator | type == "string" and test("^ghcr\\.io/lhelge/mars-orchestrator@sha256:[0-9a-f]{64}$")
images.nginx	.images.nginx | type == "string" and test("^ghcr\\.io/lhelge/mars-nginx@sha256:[0-9a-f]{64}$")
images.session_claude	.images.session_claude | type == "string" and test("^ghcr\\.io/lhelge/mars-session-claude@sha256:[0-9a-f]{64}$")
images.session_claude_dev	.images.session_claude_dev | type == "string" and test("^ghcr\\.io/lhelge/mars-session-claude-dev@sha256:[0-9a-f]{64}$")
schema.migrations	(.schema.migrations | type == "array" and length >= 1 and all(type == "string" and test("^[0-9]{14}$"))) and .schema.migrations == (.schema.migrations | sort) and (.schema.migrations | length) == (.schema.migrations | unique | length)
config.required	.config.required | type == "array" and all(type == "string" and test("^[A-Z][A-Z0-9_]*(\\|[A-Z][A-Z0-9_]*)*$"))
postgres.major	(.postgres.major | type == "number") and .postgres.major >= 1 and (.postgres.major | floor) == .postgres.major
CHECKS
)

if ! jq -e 'type == "object"' "$1" >/dev/null 2>&1; then
  echo "invalid manifest: not a JSON object" >&2
  exit 1
fi

while IFS=$'\t' read -r field expr; do
  if ! jq -e "try ($expr) catch false" "$1" >/dev/null 2>&1; then
    echo "invalid manifest: ${field}" >&2
    exit 1
  fi
done <<<"$checks"
