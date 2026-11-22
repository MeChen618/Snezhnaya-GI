#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
profile="${1:-debug}"
case "$profile" in debug|release) ;; *) echo "Usage: bash scripts/start.sh [debug|release]" >&2; exit 2 ;; esac
target="${CARGO_TARGET_DIR:-target}"
game="$target/$profile/snezhnaya-gi-game-server"
dispatch="$target/$profile/snezhnaya-gi-dispatch-server"
for binary in "$game" "$dispatch"; do
    if [[ ! -x "$binary" ]]; then
        echo "Missing $binary; build the workspace first." >&2
        exit 1
    fi
done
children=()
cleanup() {
    trap - EXIT INT TERM
    if ((${#children[@]})); then
        kill "${children[@]}" 2>/dev/null || true
        wait "${children[@]}" 2>/dev/null || true
    fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
"$game" & children+=("$!")
"$dispatch" & children+=("$!")
set +e
wait -n "${children[@]}"
status=$?
exit "$status"
