#!/bin/sh
# Validates the marketplace and every plugin: JSON parses, Claude Code's validator
# passes, and each plugin's bin/ shims are executable and start.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

status=0
fail() { echo "FAIL: $*" >&2; status=1; }

for f in $(find . -name '*.json' -not -path './.git/*'); do
  python3 -c 'import json,sys; json.load(open(sys.argv[1]))' "$f" || fail "$f is not valid JSON"
done

# Not --strict: plugins carry no version on purpose. Without one, Claude Code takes the
# git commit as the version, so every commit is an update and nothing needs bumping.
if command -v claude >/dev/null 2>&1; then
  claude plugin validate . || fail "marketplace"
  for p in plugins/*/; do
    claude plugin validate "$p" || fail "$p"
  done
else
  echo "skip: claude not installed, Claude Code validation not run" >&2
fi

# A plugin whose tool repository has resources/skills must match it; see sync-skills.sh.
tools=${TOOLS_ROOT:-$HOME/Code/github.com/joshrotenberg}
for p in plugins/*/; do
  name=$(basename "$p")
  source="$tools/$name/resources/skills"
  if [ -d "$source" ]; then
    diff -r "$source" "$p/skills" >/dev/null || fail "$name skills differ from $source; run scripts/sync-skills.sh $name"
  fi
done

for shim in plugins/*/bin/*; do
  [ -x "$shim" ] || fail "$shim is not executable"
  "$shim" --version >/dev/null || fail "$shim --version"
done

exit $status
