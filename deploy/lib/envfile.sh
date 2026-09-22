# shellcheck shell=bash
# Read an environment file the way compose does, without sourcing it
# (ARCHITECTURE.md, "Server deployment", "The manifest": nothing from the
# deployment's files is ever evaluated as shell). Sourced by the scripts in
# bin/.
#
#   declare -A ENV_VARS=()
#   ENV_BAD_LINES=()
#   read_env_file <file>      # fills ENV_VARS, and ENV_BAD_LINES with the
#                             # number of each line that is not KEY=VALUE;
#                             # returns 1 if there was one
#
# Called directly, never in $(...): a subshell would fill its own copies.
#
# Blank lines and `#` comments are skipped; one pair of matching surrounding
# quotes is removed from a value. Values are never printed.
read_env_file() {
  local file=$1 line trimmed key value line_no=0
  while IFS= read -r line || [ -n "$line" ]; do
    line_no=$((line_no + 1))
    trimmed="${line#"${line%%[![:space:]]*}"}"
    case "$trimmed" in '' | '#'*) continue ;; esac
    if ! [[ "$trimmed" =~ ^([A-Za-z_][A-Za-z0-9_]*)=(.*)$ ]]; then
      ENV_BAD_LINES+=("$line_no")
      continue
    fi
    key=${BASH_REMATCH[1]}
    value=${BASH_REMATCH[2]}
    if [[ "$value" =~ ^\"(.*)\"$ ]] || [[ "$value" =~ ^\'(.*)\'$ ]]; then
      value=${BASH_REMATCH[1]}
    fi
    # shellcheck disable=SC2034 # ENV_VARS is the caller's, declared there
    ENV_VARS["$key"]=$value
  done <"$file"
  [ "${#ENV_BAD_LINES[@]}" -eq 0 ]
}
