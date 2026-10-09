#!/bin/sh
set -eu

tosi_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
exec cargo run --manifest-path "$tosi_root/xtask/Cargo.toml" --package xtask -- package "$@"
