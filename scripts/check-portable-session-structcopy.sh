#!/bin/sh
# FEAT-026: real production source, shared outcomes and nonzero portable tests.
set -eu
cd "$(dirname "$0")/.."
python3 scripts/check-command-structcopy-proof.py
graph=$(cargo tree --locked -p codewhale-portable-session-structcopy --edges normal --prefix none)
printf '%s\n' "$graph"
if printf '%s\n' "$graph" | grep -Eq '^codewhale-tui([[:space:]]|$)'; then
    echo '[portable-session-structcopy] FAIL: TUI is a normal dependency' >&2
    exit 1
fi
cargo check --locked -p codewhale-portable-session-structcopy --lib --no-default-features
sh scripts/with-hermetic-test-home.sh cargo nextest run -p codewhale-portable-session-structcopy --lib --all-features --locked --profile ci --no-tests=fail
