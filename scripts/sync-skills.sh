#!/bin/sh
# Copies skills from each tool's crate into its plugin, so a tool that also serves its
# skills as MCP resources has one source. A plugin whose crate has no resources/skills
# directory keeps the skills written in the plugin.
#
#   scripts/sync-skills.sh            every plugin
#   scripts/sync-skills.sh tosi       one plugin
#
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
tools="$root/crates"

plugins=${*:-$(ls "$root/plugins")}
for name in $plugins; do
  source="$tools/$name/resources/skills"
  if [ ! -d "$source" ]; then
    echo "skip: $name has no $source"
    continue
  fi
  rm -rf "$root/plugins/$name/skills"
  mkdir -p "$root/plugins/$name/skills"
  cp -R "$source/." "$root/plugins/$name/skills/"
  echo "synced: $name from $source"
done
